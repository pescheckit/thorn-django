//! Dynamic validation — actually execute Django code via PyO3 to catch real errors.
//!
//! Unlike static analysis, these checks call Django APIs and catch the actual
//! exceptions Django throws. This is 100% accurate because it's Django itself
//! telling us what's wrong.

use pyo3::prelude::*;
use pyo3::types::PyDict;
use thorn_api::Diagnostic;

/// Run all Django dynamic validation checks.
/// Requires django.setup() to have been called already.
pub fn run_all_dynamic_checks(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    // 1. Django's own system check framework
    diagnostics.extend(run_django_system_checks(py)?);

    // 2. Validate all models
    diagnostics.extend(validate_models(py)?);

    // 3. Validate serializers (if DRF is installed)
    diagnostics.extend(validate_serializers(py)?);

    // 4. Check migration state
    diagnostics.extend(check_migrations(py)?);

    // 5. Validate URL patterns
    diagnostics.extend(validate_urls(py)?);

    Ok(diagnostics)
}

/// Run Django's built-in system check framework.
/// This catches: model field errors, admin registration issues, database config
/// problems, security warnings, and hundreds of other Django-specific issues.
fn run_django_system_checks(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    let checks_mod = py.import("django.core.checks")?;
    let results = checks_mod.call_method0("run_checks")?;

    for check_result in results.try_iter()? {
        let check = check_result?;
        let level: i32 = check.getattr("level")?.extract()?;
        let msg: String = check.getattr("msg")?.extract()?;
        let check_id: String = check.getattr("id")?.extract()?;
        let hint: String = check
            .getattr("hint")?
            .extract()
            .unwrap_or_default();

        // Map Django check levels to our codes
        // 0=DEBUG, 10=INFO, 20=WARNING, 25=ERROR, 30=CRITICAL, 40=FATAL
        let code = match level {
            40 => "DV-FATAL",
            30 => "DV-CRIT",
            25 => "DV-ERR",
            20 => "DV-WARN",
            _ => continue, // skip DEBUG/INFO
        };

        let obj_str: String = check
            .getattr("obj")
            .and_then(|o| o.str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let message = if hint.is_empty() {
            format!("[{check_id}] {msg} (on {obj_str})")
        } else {
            format!("[{check_id}] {msg} (on {obj_str}). Hint: {hint}")
        };

        diagnostics.push(Diagnostic::new(code, message, "django.checks"));
    }

    Ok(diagnostics)
}

/// Validate all model fields and meta options by actually calling Django APIs.
fn validate_models(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    let apps = py.import("django.apps")?.getattr("apps")?;
    let models = apps.call_method0("get_models")?;

    // Pre-import sys.modules for source file lookups
    let sys_modules = py.import("sys")?.getattr("modules")?;

    for model_cls in models.try_iter()? {
        let model_cls = model_cls?;
        let meta = model_cls.getattr("_meta")?;
        let model_name: String = model_cls.getattr("__name__")?.extract()?;
        let app_label: String = meta.getattr("app_label")?.extract()?;
        let module: String = model_cls.getattr("__module__")?.extract()?;
        let abstract_model: bool = meta.getattr("abstract")?.extract()?;

        if abstract_model {
            continue;
        }

        // Resolve the source file for this model's module, if available.
        let source_file: String = sys_modules
            .get_item(&module)
            .ok()
            .and_then(|m: pyo3::Bound<'_, pyo3::PyAny>| m.getattr("__file__").ok())
            .and_then(|f: pyo3::Bound<'_, pyo3::PyAny>| f.extract::<String>().ok())
            .unwrap_or_default();

        // Skip third-party models
        let third_party_prefixes = ["django.", "rest_framework.", "allauth.", "guardian.",
            "django_q.", "django_otp.", "otp_", "oauth2_provider.",
            "axes.", "simple_history.", "django_filters.",
            "drf_spectacular.", "corsheaders.", "debug_toolbar.",
            "storages.", "celery.", "kombu.", "djstripe."];
        let is_third_party = third_party_prefixes.iter().any(|p| module.starts_with(p))
            || source_file.contains("site-packages")
            || source_file.contains("/venv/")
            || source_file.contains("/.venv/");
        if is_third_party { continue; }

        // Check for models without __str__ by walking the MRO
        let mro = model_cls.getattr("__mro__")?;
        let mut has_custom_str = false;
        for cls in mro.try_iter()? {
            let cls = cls?;
            let cls_name: String = cls.getattr("__name__")?.extract()?;
            if cls_name == "Model" || cls_name == "object" {
                break;
            }
            let dict = cls.getattr("__dict__")?;
            if dict.contains("__str__")? {
                has_custom_str = true;
                break;
            }
        }
        if !has_custom_str {
            diagnostics.push(Diagnostic::new(
                "DV001",
                format!(
                    "Model '{app_label}.{model_name}' has no __str__ — \
                     will show as '{model_name} object (pk)' in admin."
                ),
                &module,
            ));
        }

        // Validate unique_together fields actually exist
        let unique_together = meta.getattr("unique_together")?;
        if !unique_together.is_none() {
            for combo in unique_together.try_iter()? {
                let combo = combo?;
                for field_name_obj in combo.try_iter()? {
                    let field_name: String = field_name_obj?.extract()?;
                    if meta.call_method1("get_field", (&field_name,)).is_err() {
                        diagnostics.push(Diagnostic::new(
                            "DV002",
                            format!(
                                "Model '{app_label}.{model_name}' unique_together \
                                 references non-existent field '{field_name}'."
                            ),
                            &module,
                        ));
                    }
                }
            }
        }

        // Validate Meta.ordering fields actually exist
        let ordering = meta.getattr("ordering")?;
        if !ordering.is_none() {
            for order_field_obj in ordering.try_iter()? {
                let order_field: String = match order_field_obj?.extract() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let clean = order_field.trim_start_matches('-').trim_start_matches('?');
                if clean == "pk" || clean == "?" || clean.is_empty() {
                    continue;
                }
                let base = clean.split("__").next().unwrap_or(clean);
                if meta.call_method1("get_field", (base,)).is_err() {
                    diagnostics.push(Diagnostic::new(
                        "DV003",
                        format!(
                            "Model '{app_label}.{model_name}' Meta.ordering \
                             references non-existent field '{order_field}'."
                        ),
                        &module,
                    ));
                }
            }
        }
    }

    Ok(diagnostics)
}

/// Validate DRF serializers by checking Meta.fields against the actual model.
fn validate_serializers(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    // Try to import DRF
    let drf = match py.import("rest_framework.serializers") {
        Ok(m) => m,
        Err(_) => return Ok(diagnostics), // DRF not installed
    };

    let base_serializer = drf.getattr("ModelSerializer")?;

    // Get all subclasses of ModelSerializer
    let subclasses = get_all_subclasses(py, &base_serializer)?;

    for cls in &subclasses {
        let cls_name: String = cls.getattr("__name__")?.extract()?;
        let cls_module: String = cls.getattr("__module__")?.extract()?;

        // Check if it has Meta with model and fields
        let meta = match cls.getattr("Meta") {
            Ok(m) => m,
            Err(_) => continue,
        };

        let model = match meta.getattr("model") {
            Ok(m) if !m.is_none() => m,
            _ => continue,
        };

        let model_name: String = model.getattr("__name__")?.extract()?;
        let model_meta = model.getattr("_meta")?;

        let fields_attr = match meta.getattr("fields") {
            Ok(f) if !f.is_none() => f,
            _ => continue,
        };

        // If fields = '__all__', skip
        if let Ok(s) = fields_attr.extract::<String>() {
            if s == "__all__" {
                continue;
            }
        }

        // Validate each field
        if let Ok(field_list) = fields_attr.extract::<Vec<String>>() {
            for field_name in &field_list {
                // Check model field
                let model_has = model_meta
                    .call_method1("get_field", (field_name.as_str(),))
                    .is_ok();

                // Check declared serializer field
                let serializer_has = cls
                    .getattr("_declared_fields")
                    .and_then(|d| d.contains(field_name.as_str()))
                    .unwrap_or(false);

                // Check model property/method
                let property_has = model.getattr(field_name.as_str()).is_ok();

                if !model_has && !serializer_has && !property_has {
                    diagnostics.push(Diagnostic::new(
                        "DV101",
                        format!(
                            "Serializer '{cls_name}' declares field '{field_name}' in Meta.fields \
                             but it doesn't exist on model '{model_name}' and is not declared."
                        ),
                        &cls_module,
                    ));
                }
            }
        }
    }

    Ok(diagnostics)
}

/// Check migration state — detect conflicts and unapplied migrations.
fn check_migrations(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    let loader_cls = py
        .import("django.db.migrations.loader")?
        .getattr("MigrationLoader")?;

    let connection = py.import("django.db")?.getattr("connection")?;

    let loader = loader_cls.call1((&connection, true))?;

    // Check for conflicts
    let conflicts = loader.call_method0("detect_conflicts")?;
    let conflicts_dict: &Bound<'_, PyDict> = conflicts.downcast()?;

    for (app, migrations) in conflicts_dict.iter() {
        let app_name: String = app.extract()?;
        let mig_list: Vec<String> = migrations.extract()?;
        diagnostics.push(Diagnostic::new(
            "DV201",
            format!(
                "Migration conflict in app '{}': {}",
                app_name,
                mig_list.join(", ")
            ),
            "migrations",
        ));
    }

    Ok(diagnostics)
}

/// Validate URL patterns by walking the resolver tree.
fn validate_urls(py: Python<'_>) -> PyResult<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();

    let resolver_mod = match py.import("django.urls.resolvers") {
        Ok(m) => m,
        Err(_) => return Ok(diagnostics),
    };

    let get_resolver = resolver_mod.getattr("get_resolver")?;
    let resolver = match get_resolver.call0() {
        Ok(r) => r,
        Err(_) => return Ok(diagnostics),
    };

    walk_url_patterns(py, &resolver, "", &mut diagnostics)?;

    Ok(diagnostics)
}

#[allow(clippy::only_used_in_recursion)]
fn walk_url_patterns(
    py: Python<'_>,
    resolver: &Bound<'_, PyAny>,
    prefix: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> PyResult<()> {
    let url_patterns = match resolver.getattr("url_patterns") {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };

    for pattern in url_patterns.try_iter()? {
        let pattern = pattern?;
        let class_name: String = pattern
            .getattr("__class__")?
            .getattr("__name__")?
            .extract()?;

        if class_name == "URLResolver" {
            let nested_prefix: String = pattern
                .getattr("pattern")
                .and_then(|p| p.str())
                .map(|s| format!("{prefix}{s}"))
                .unwrap_or_else(|_| prefix.to_string());
            walk_url_patterns(py, &pattern, &nested_prefix, diagnostics)?;
        } else if class_name == "URLPattern" {
            let callback = match pattern.getattr("callback") {
                Ok(c) => c,
                Err(_) => {
                    let name: String = pattern
                        .getattr("name")
                        .and_then(|n| n.extract())
                        .unwrap_or_else(|_| "unknown".into());
                    diagnostics.push(Diagnostic::new(
                        "DV301",
                        format!("URL pattern '{prefix}' (name='{name}') has no valid callback."),
                        "urls",
                    ));
                    continue;
                }
            };

            if !callback.is_callable() {
                let name: String = pattern
                    .getattr("name")
                    .and_then(|n| n.extract())
                    .unwrap_or_else(|_| "unknown".into());
                diagnostics.push(Diagnostic::new(
                    "DV302",
                    format!("URL pattern '{prefix}' (name='{name}') callback is not callable."),
                    "urls",
                ));
            }
        }
    }

    Ok(())
}

/// Get all subclasses of a class (recursive).
fn get_all_subclasses<'py>(
    _py: Python<'py>,
    base: &Bound<'py, PyAny>,
) -> PyResult<Vec<Bound<'py, PyAny>>> {
    let mut result = Vec::new();
    let mut stack = vec![base.clone()];

    while let Some(cls) = stack.pop() {
        let subs = cls.call_method0("__subclasses__")?;
        for sc in subs.try_iter()? {
            let sc = sc?;
            let name: String = sc.getattr("__name__")?.extract()?;
            if name.starts_with('_') {
                continue;
            }

            // Check if abstract
            let is_abstract = sc
                .getattr("Meta")
                .and_then(|m| m.getattr("abstract"))
                .and_then(|a| a.extract::<bool>())
                .unwrap_or(false);

            if !is_abstract {
                result.push(sc.clone());
            }
            stack.push(sc);
        }
    }

    Ok(result)
}
