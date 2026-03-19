//! Cross-referencing checks: combine AST analysis with the live AppGraph.

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::*;
use ruff_text_size::Ranged;
use thorn_api::{AstCheck, CheckContext, Diagnostic};

/// Check if a model is from a third-party package (incomplete schema, can't validate fields).
fn is_third_party_model(model: &thorn_api::Model) -> bool {
    if !model.source_file.is_empty() {
        return model.source_file.contains("site-packages")
            || model.source_file.contains("/venv/")
            || model.source_file.contains("/.venv/");
    }
    const THIRD_PARTY: &[&str] = &[
        "django.",
        "rest_framework.",
        "allauth.",
        "guardian.",
        "django_q.",
        "django_otp.",
        "otp_",
        "oauth2_provider.",
        "axes.",
        "simple_history.",
        "django_filters.",
        "drf_spectacular.",
        "corsheaders.",
        "debug_toolbar.",
        "storages.",
        "celery.",
        "kombu.",
        "djstripe.",
    ];
    THIRD_PARTY
        .iter()
        .any(|prefix| model.module.starts_with(prefix))
}

// ── DJ201: Invalid field in .filter()/.exclude() ─────────────────────────

pub struct InvalidFilterField;

impl AstCheck for InvalidFilterField {
    fn code(&self) -> &'static str {
        "DJ201"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = FilterFieldVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct FilterFieldVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

const QUERYSET_FILTER_METHODS: &[&str] = &[
    "filter",
    "exclude",
    "get",
    "get_or_create",
    "update_or_create",
    "update",
    "create",
];

impl<'a, 'g> Visitor<'a> for FilterFieldVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            if let Some((model_name, method_name)) = extract_model_queryset_call(&call.func) {
                if QUERYSET_FILTER_METHODS.contains(&method_name.as_str()) {
                    let candidates = self.graph.find_models_by_name(&model_name);
                    if candidates.is_empty() || candidates.iter().all(|m| is_third_party_model(m)) {
                        visitor::walk_expr(self, expr);
                        return;
                    }
                    // Collect annotation names from the queryset chain
                    let annotations = collect_annotation_names(&call.func);
                    for kw in &call.arguments.keywords {
                        if let Some(arg_name) = &kw.arg {
                            let field_name = arg_name.as_str();
                            let base_field = field_name.split("__").next().unwrap_or(field_name);
                            if base_field == "pk" || base_field == "defaults" {
                                continue;
                            }
                            // Skip fields introduced by .annotate() in the chain
                            if annotations.contains(&base_field.to_string()) {
                                continue;
                            }
                            let found = candidates
                                .iter()
                                .any(|m| model_has_field_flexible(m, base_field));
                            if !found {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        "DJ201",
                                        format!(
                                            "Field '{field_name}' does not exist on model '{}'.",
                                            model_name
                                        ),
                                        &self.filename,
                                    )
                                    .with_range(kw.range()),
                                );
                            }
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ202: Invalid field in .values()/.order_by() ────────────────────────

pub struct InvalidValuesField;

impl AstCheck for InvalidValuesField {
    fn code(&self) -> &'static str {
        "DJ202"
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = ValuesFieldVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct ValuesFieldVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

const QUERYSET_STRING_ARG_METHODS: &[&str] =
    &["values", "values_list", "order_by", "only", "defer"];

impl<'a, 'g> Visitor<'a> for ValuesFieldVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            if let Some((model_name, method_name)) = extract_model_queryset_call(&call.func) {
                if QUERYSET_STRING_ARG_METHODS.contains(&method_name.as_str()) {
                    let candidates = self.graph.find_models_by_name(&model_name);
                    if candidates.is_empty() || candidates.iter().all(|m| is_third_party_model(m)) {
                        visitor::walk_expr(self, expr);
                        return;
                    }
                    for arg in &call.arguments.args {
                        if let Expr::StringLiteral(s) = arg {
                            let field_name = s.value.to_str();
                            let base_field = field_name.split("__").next().unwrap_or(field_name);
                            let base_field = base_field.strip_prefix('-').unwrap_or(base_field);
                            if base_field == "pk" {
                                continue;
                            }
                            let found = candidates
                                .iter()
                                .any(|m| model_has_field_flexible(m, base_field));
                            if !found {
                                self.diagnostics.push(Diagnostic::new("DJ202", format!("Field '{field_name}' passed to .{method_name}() does not exist on model '{}'.", model_name), &self.filename).with_range(arg.range()));
                            }
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ203: Nonexistent manager method ────────────────────────────────────

pub struct InvalidManagerMethod;

impl AstCheck for InvalidManagerMethod {
    fn code(&self) -> &'static str {
        "DJ203"
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = ManagerMethodVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct ManagerMethodVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

const BUILTIN_MANAGER_METHODS: &[&str] = &[
    "none",
    "all",
    "count",
    "dates",
    "datetimes",
    "distinct",
    "extra",
    "get",
    "get_or_create",
    "update_or_create",
    "get_queryset",
    "create",
    "bulk_create",
    "bulk_update",
    "filter",
    "aggregate",
    "annotate",
    "complex_filter",
    "exclude",
    "in_bulk",
    "iterator",
    "latest",
    "earliest",
    "first",
    "last",
    "order_by",
    "select_for_update",
    "select_related",
    "prefetch_related",
    "values",
    "values_list",
    "update",
    "reverse",
    "defer",
    "only",
    "using",
    "exists",
    "delete",
    "as_manager",
    "raw",
    "explain",
    "union",
    "intersection",
    "difference",
    "aiterator",
    "aget",
    "acreate",
    "aget_or_create",
    "aupdate_or_create",
    "abulk_create",
    "abulk_update",
    "acount",
    "ain_bulk",
    "alatest",
    "aearliest",
    "afirst",
    "alast",
    "aaggregate",
    "aexists",
    "aupdate",
    "adelete",
    "acontains",
    "contribute_to_class",
    "db_manager",
    "db",
    "auto_created",
    "use_in_migrations",
    "model",
];

impl<'a, 'g> Visitor<'a> for ManagerMethodVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if let Expr::Attribute(inner_attr) = attr.value.as_ref() {
                    let manager_name = inner_attr.attr.as_str();
                    let method_name = attr.attr.as_str();
                    if let Expr::Name(model_ref) = inner_attr.value.as_ref() {
                        if let Some(model) = self.graph.find_model_by_name(model_ref.id.as_str()) {
                            if let Some(manager) =
                                model.managers.iter().find(|m| m.name == manager_name)
                            {
                                if !BUILTIN_MANAGER_METHODS.contains(&method_name)
                                    && !manager.custom_methods.contains(&method_name.to_string())
                                {
                                    self.diagnostics.push(Diagnostic::new("DJ203", format!("Method '{method_name}' does not exist on manager '{manager_name}' of model '{}'.", model.name), &self.filename).with_range(attr.range()));
                                }
                            }
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ204: get_FOO_display() on field without choices ────────────────────

pub struct InvalidGetDisplay;

impl AstCheck for InvalidGetDisplay {
    fn code(&self) -> &'static str {
        "DJ204"
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = GetDisplayVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct GetDisplayVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

impl<'a, 'g> Visitor<'a> for GetDisplayVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                let method = attr.attr.as_str();
                if method.starts_with("get_") && method.ends_with("_display") {
                    let field_name = &method[4..method.len() - 8];
                    if !field_name.is_empty() {
                        for model in &self.graph.models {
                            if let Some(field) = model.get_field(field_name) {
                                if field.choices.is_empty() {
                                    self.diagnostics.push(Diagnostic::new("DJ204", format!("get_{field_name}_display() called but field '{field_name}' on model '{}' has no choices.", model.name), &self.filename).with_range(expr.range()));
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ205: Serializer references nonexistent model field ─────────────────

pub struct SerializerFieldMismatch;

impl AstCheck for SerializerFieldMismatch {
    fn code(&self) -> &'static str {
        "DJ205"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = SerializerFieldVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct SerializerFieldVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

impl<'a, 'g> Visitor<'a> for SerializerFieldVisitor<'g> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::ClassDef(class) = stmt {
            if is_serializer(class) {
                if let Some((model_name, field_names)) = extract_meta_model_and_fields(class) {
                    let candidates = self.graph.find_models_by_name(&model_name);
                    if candidates.is_empty() || candidates.iter().all(|m| is_third_party_model(m)) {
                        visitor::walk_stmt(self, stmt);
                        return;
                    }
                    // Collect explicitly declared fields and get_X methods on the serializer class
                    let declared = collect_declared_serializer_fields(class);
                    for (field_name, range) in &field_names {
                        if field_name == "__all__" {
                            continue;
                        }
                        if declared.contains(field_name.as_str()) {
                            continue;
                        }
                        // Field is valid if it exists on ANY model with this name
                        let found = candidates.iter().any(|m| {
                            m.has_field_or_relation(field_name) || m.has_method(field_name)
                        });
                        if !found && field_name != "pk" {
                            self.diagnostics.push(Diagnostic::new("DJ205", format!("Serializer '{}' references field '{}' which doesn't exist on model '{}'.", class.name, field_name, model_name), &self.filename).with_range(*range));
                        }
                    }
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ206: Wrong reverse accessor ────────────────────────────────────────

pub struct WrongReverseAccessor;

impl AstCheck for WrongReverseAccessor {
    fn code(&self) -> &'static str {
        "DJ206"
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = ReverseAccessorVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct ReverseAccessorVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

impl<'a, 'g> Visitor<'a> for ReverseAccessorVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Attribute(attr) = expr {
            let accessor = attr.attr.as_str();
            if let Some(model_name_lower) = accessor.strip_suffix("_set") {
                for model in &self.graph.models {
                    for rel in &model.relations {
                        if matches!(
                            rel.kind,
                            thorn_api::RelationKind::ForeignKey | thorn_api::RelationKind::OneToOne
                        ) && model.name.to_lowercase() == model_name_lower
                            && !rel.related_name.is_empty()
                            && rel.related_name != accessor
                            && rel.related_name != "+"
                        {
                            self.diagnostics.push(Diagnostic::new("DJ206", format!("Default reverse accessor '{accessor}' won't work — the FK from '{}' to '{}' has related_name='{}'. Use '.{}' instead.", model.name, rel.to_model, rel.related_name, rel.related_name), &self.filename).with_range(expr.range()));
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ207: self.foreignkey.id instead of self.foreignkey_id ──────────────

pub struct ForeignKeyIdAccess;

impl AstCheck for ForeignKeyIdAccess {
    fn code(&self) -> &'static str {
        "DJ207"
    }
    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.graph.models.is_empty() {
            return vec![];
        }
        let mut v = FKIdVisitor {
            diagnostics: vec![],
            filename: ctx.filename.to_string(),
            graph: ctx.graph,
        };
        v.visit_body(&ctx.module.body);
        v.diagnostics
    }
}

struct FKIdVisitor<'g> {
    diagnostics: Vec<Diagnostic>,
    filename: String,
    graph: &'g thorn_api::AppGraph,
}

impl<'a, 'g> Visitor<'a> for FKIdVisitor<'g> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Attribute(outer) = expr {
            if matches!(outer.attr.as_str(), "id" | "pk") {
                if let Expr::Attribute(inner) = outer.value.as_ref() {
                    let field_name = inner.attr.as_str();
                    let is_self =
                        matches!(inner.value.as_ref(), Expr::Name(n) if n.id.as_str() == "self");
                    if is_self {
                        for model in &self.graph.models {
                            for rel in &model.relations {
                                if rel.name == field_name
                                    && matches!(
                                        rel.kind,
                                        thorn_api::RelationKind::ForeignKey
                                            | thorn_api::RelationKind::OneToOne
                                    )
                                {
                                    self.diagnostics.push(Diagnostic::new("DJ207", format!("'self.{field_name}.{}' triggers a DB query. Use 'self.{field_name}_id' — reads the cached column directly.", outer.attr.as_str()), &self.filename).with_range(expr.range()));
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn extract_model_queryset_call(expr: &Expr) -> Option<(String, String)> {
    if let Expr::Attribute(attr) = expr {
        let method_name = attr.attr.as_str().to_string();
        if let Expr::Attribute(inner) = attr.value.as_ref() {
            if inner.attr.as_str() == "objects" || inner.attr.as_str() == "all_objects" {
                if let Expr::Name(model) = inner.value.as_ref() {
                    return Some((model.id.to_string(), method_name));
                }
            }
            if let Expr::Call(chain_call) = inner.value.as_ref() {
                if let Some((model, _)) = extract_model_queryset_call(&chain_call.func) {
                    return Some((model, method_name));
                }
            }
        }
        if let Expr::Call(call) = attr.value.as_ref() {
            if let Some((model, _)) = extract_model_queryset_call(&call.func) {
                return Some((model, method_name));
            }
        }
    }
    None
}

fn is_serializer(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| match base {
            Expr::Attribute(a) => matches!(
                a.attr.as_str(),
                "ModelSerializer" | "Serializer" | "HyperlinkedModelSerializer"
            ),
            Expr::Name(n) => matches!(
                n.id.as_str(),
                "ModelSerializer" | "Serializer" | "HyperlinkedModelSerializer"
            ),
            _ => false,
        })
    })
}

fn extract_meta_model_and_fields(
    class: &StmtClassDef,
) -> Option<(String, Vec<(String, ruff_text_size::TextRange)>)> {
    let mut model_name = None;
    let mut field_names = Vec::new();

    for stmt in &class.body {
        if let Stmt::ClassDef(meta) = stmt {
            if meta.name.as_str() != "Meta" {
                continue;
            }
            for meta_stmt in &meta.body {
                if let Stmt::Assign(assign) = meta_stmt {
                    for target in &assign.targets {
                        if let Expr::Name(n) = target {
                            match n.id.as_str() {
                                "model" => {
                                    model_name = extract_name_from_expr(&assign.value);
                                }
                                "fields" => {
                                    if let Expr::List(list) = assign.value.as_ref() {
                                        for elt in &list.elts {
                                            if let Expr::StringLiteral(s) = elt {
                                                field_names.push((
                                                    s.value.to_str().to_string(),
                                                    elt.range(),
                                                ));
                                            }
                                        }
                                    } else if let Expr::Tuple(tuple) = assign.value.as_ref() {
                                        for elt in &tuple.elts {
                                            if let Expr::StringLiteral(s) = elt {
                                                field_names.push((
                                                    s.value.to_str().to_string(),
                                                    elt.range(),
                                                ));
                                            }
                                        }
                                    } else if let Expr::StringLiteral(s) = assign.value.as_ref() {
                                        if s.value.to_str() == "__all__" {
                                            field_names.push(("__all__".into(), assign.range()));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
    model_name.map(|m| (m, field_names))
}

/// Walk the queryset call chain to collect annotation names from .annotate() calls.
/// e.g., `Model.objects.filter(...).annotate(count=Count(...)).filter(count__gt=0)`
/// returns ["count"].
fn collect_annotation_names(expr: &Expr) -> Vec<String> {
    let mut names = Vec::new();
    collect_annotations_recursive(expr, &mut names);
    names
}

fn collect_annotations_recursive(expr: &Expr, names: &mut Vec<String>) {
    if let Expr::Attribute(attr) = expr {
        if let Expr::Call(call) = attr.value.as_ref() {
            // Check if the call is to .annotate()
            if let Expr::Attribute(inner_attr) = call.func.as_ref() {
                if inner_attr.attr.as_str() == "annotate" {
                    for kw in &call.arguments.keywords {
                        if let Some(arg_name) = &kw.arg {
                            names.push(arg_name.to_string());
                        }
                    }
                }
                // Continue walking the chain
                collect_annotations_recursive(&call.func, names);
            }
        }
    }
}

/// Flexible field lookup that handles Django's implicit columns and virtual fields.
fn model_has_field_flexible(model: &thorn_api::Model, name: &str) -> bool {
    if model.has_field_or_relation(name) {
        return true;
    }
    // ForeignKey _id column: "organisation_id" → relation "organisation"
    if let Some(relation_name) = name.strip_suffix("_id") {
        if model.relations.iter().any(|r| r.name == relation_name) {
            return true;
        }
    }
    // django-modeltranslation virtual field: "description_i18n" → field "description"
    if let Some(base_name) = name.strip_suffix("_i18n") {
        if model.has_field_or_relation(base_name) {
            return true;
        }
    }
    false
}

fn extract_name_from_expr(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.id.to_string()),
        Expr::Attribute(a) => Some(a.attr.to_string()),
        _ => None,
    }
}

/// Collect field names that are explicitly declared on a serializer class.
/// This includes:
///   - Class-level assignments: `field_name = serializers.CharField(...)`
///   - Fields covered by a `get_X` method (SerializerMethodField pattern)
///   - Fields handled in `to_representation` (we can't easily detect which, so we just
///     check for declared attributes and get_ methods)
fn collect_declared_serializer_fields(class: &StmtClassDef) -> std::collections::HashSet<&str> {
    let mut declared = std::collections::HashSet::new();
    for stmt in &class.body {
        match stmt {
            // Class-level field declarations: `field_name = ...`
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        declared.insert(n.id.as_str());
                    }
                }
            }
            // Annotated assignments: `field_name: Type = ...`
            Stmt::AnnAssign(ann) => {
                if let Expr::Name(n) = ann.target.as_ref() {
                    declared.insert(n.id.as_str());
                }
            }
            // Methods: get_X covers SerializerMethodField for field X
            Stmt::FunctionDef(func) => {
                let name = func.name.as_str();
                if let Some(field_name) = name.strip_prefix("get_") {
                    declared.insert(field_name);
                }
            }
            _ => {}
        }
    }
    declared
}
