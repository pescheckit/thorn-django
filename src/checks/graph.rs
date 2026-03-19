use std::collections::HashMap;
use thorn_api::{Diagnostic, GraphCheck, Model, ModelGraph, RelationKind};

/// Check if a model is auto-generated (e.g., django-simple-history Historical* models).
/// These models can't be directly edited and shouldn't be flagged.
fn is_auto_generated(model: &Model) -> bool {
    // django-simple-history generates HistoricalX for each model X
    if model.name.starts_with("Historical") {
        return model.module.contains("simple_history")
            || model.source_file.ends_with("__init__.py");
    }
    false
}

/// Check if a model is first-party (not from a third-party package).
fn is_first_party(model: &Model) -> bool {
    if !model.source_file.is_empty() {
        return !model.source_file.contains("site-packages")
            && !model.source_file.contains("/venv/")
            && !model.source_file.contains("/.venv/");
    }
    let third_party = [
        "django.", "rest_framework.", "allauth.", "guardian.",
        "django_q.", "django_otp.", "otp_", "oauth2_provider.",
        "axes.", "simple_history.", "django_filters.",
        "drf_spectacular.", "corsheaders.", "debug_toolbar.",
        "storages.", "celery.", "kombu.", "djstripe.",
    ];
    !third_party.iter().any(|prefix| model.module.starts_with(prefix))
}

fn model_filename(model: &Model) -> &str {
    if model.source_file.is_empty() { &model.module } else { &model.source_file }
}

// ── DJ101: Model missing __str__ ─────────────────────────────────────────

pub struct GraphModelMissingStr;

impl GraphCheck for GraphModelMissingStr {
    fn code(&self) -> &'static str { "DJ101" }

    fn check(&self, graph: &ModelGraph) -> Vec<Diagnostic> {
        graph.models.iter()
            .filter(|m| !m.abstract_model && !m.proxy)
            .filter(|m| is_first_party(m))
            .filter(|m| !is_auto_generated(m))
            .filter(|m| !m.has_method("__str__"))
            .map(|m| {
                let mut d = Diagnostic::new(
                    "DJ101",
                    format!("Model '{}.{}' is missing __str__ — shows as '{} object (pk)' in admin/forms.", m.app_label, m.name, m.name),
                    model_filename(m),
                );
                d.line = find_class_line(model_filename(m), &m.name);
                d
            })
            .collect()
    }
}

// ── DJ102: Duplicate related_name ────────────────────────────────────────

pub struct DuplicateRelatedName;

impl GraphCheck for DuplicateRelatedName {
    fn code(&self) -> &'static str { "DJ102" }

    #[allow(clippy::type_complexity)]
    fn check(&self, graph: &ModelGraph) -> Vec<Diagnostic> {
        let mut seen: HashMap<(&str, &str, &str), Vec<(&str, &str, &str)>> = HashMap::new();

        for model in &graph.models {
            if !is_first_party(model) { continue; }
            for rel in &model.relations {
                if rel.related_name.is_empty() { continue; }
                if matches!(rel.kind, RelationKind::Reverse | RelationKind::ReverseOneToOne) { continue; }
                seen.entry((&rel.to_model_app, &rel.to_model, &rel.related_name))
                    .or_default()
                    .push((&model.app_label, &model.name, model_filename(model)));
            }
        }

        seen.values()
            .filter(|sources| sources.len() > 1)
            .flat_map(|sources| {
                let models: Vec<_> = sources.iter().map(|(app, name, _)| format!("{app}.{name}")).collect();
                sources.iter().map(move |(app, name, filename)| {
                    Diagnostic::new("DJ102", format!("Duplicate related_name on '{app}.{name}' — conflicts with: {}", models.join(", ")), *filename)
                })
            })
            .collect()
    }
}

// ── DJ103: null=True on string-based fields ──────────────────────────────

const STRING_FIELD_TYPES: &[&str] = &[
    "CharField", "TextField", "EmailField", "URLField",
    "SlugField", "FilePathField", "FileField", "ImageField",
];

pub struct NullableStringFieldGraph;

impl GraphCheck for NullableStringFieldGraph {
    fn code(&self) -> &'static str { "DJ103" }

    fn check(&self, graph: &ModelGraph) -> Vec<Diagnostic> {
        graph.models.iter()
            .filter(|m| is_first_party(m))
            .filter(|m| !is_auto_generated(m))
            .flat_map(|m| {
                m.fields.iter().filter_map(move |f| {
                    let is_string = STRING_FIELD_TYPES.iter().any(|t| f.field_class.contains(t));
                    if is_string && f.nullable && !f.unique {
                        let mut d = Diagnostic::new(
                            "DJ103",
                            format!("{}.{}.{} — null=True on string field. Use blank=True instead.", m.app_label, m.name, f.name),
                            model_filename(m),
                        );
                        d.line = find_field_line(model_filename(m), &f.name);
                        Some(d)
                    } else {
                        None
                    }
                })
            })
            .collect()
    }
}

// ── DJ104: FK target model not found ─────────────────────────────────────

pub struct MissingReverseAccessor;

impl GraphCheck for MissingReverseAccessor {
    fn code(&self) -> &'static str { "DJ104" }

    fn check(&self, graph: &ModelGraph) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        for model in &graph.models {
            if !is_first_party(model) { continue; }
            for rel in &model.relations {
                if matches!(rel.kind, RelationKind::Reverse | RelationKind::ReverseOneToOne) { continue; }
                if graph.get_model(&rel.to_model_app, &rel.to_model).is_none() {
                    diagnostics.push(Diagnostic::new(
                        "DJ104",
                        format!("ForeignKey '{}.{}.{}' points to '{}.{}' which is not in the model graph.", model.app_label, model.name, rel.name, rel.to_model_app, rel.to_model),
                        model_filename(model),
                    ));
                }
            }
        }
        diagnostics
    }
}

// ── Source location helpers ──────────────────────────────────────────────

fn find_class_line(filename: &str, class_name: &str) -> Option<u32> {
    let source = std::fs::read_to_string(filename).ok()?;
    let pattern = format!("class {class_name}");
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&pattern) && trimmed[pattern.len()..].starts_with(['(', ':']) {
            return Some((i + 1) as u32);
        }
    }
    None
}

fn find_field_line(filename: &str, field_name: &str) -> Option<u32> {
    let source = std::fs::read_to_string(filename).ok()?;
    let pattern = format!("{field_name} = ");
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&pattern) {
            return Some((i + 1) as u32);
        }
    }
    None
}
