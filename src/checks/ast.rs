use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::*;
use ruff_text_size::Ranged;
use thorn_api::{AstCheck, CheckContext, Diagnostic};

// ── DJ001: NullableStringField ────────────────────────────────────────────

pub struct NullableStringField;

impl AstCheck for NullableStringField {
    fn code(&self) -> &'static str {
        "DJ001"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = NullableStringFieldVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct NullableStringFieldVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

const STRING_FIELD_NAMES: &[&str] = &[
    "CharField",
    "TextField",
    "EmailField",
    "URLField",
    "SlugField",
    "FilePathField",
    "FileField",
    "ImageField",
    // GenericIPAddressField and IPAddressField legitimately use null=True per Django docs
    // (they store NULL in the DB for "no address" rather than an empty string)
];

impl<'a> Visitor<'_> for NullableStringFieldVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let fn_name = match call.func.as_ref() {
                Expr::Name(n) => Some(n.id.as_str().to_string()),
                Expr::Attribute(a) => Some(a.attr.as_str().to_string()),
                _ => None,
            };
            if let Some(name) = fn_name {
                if STRING_FIELD_NAMES.contains(&name.as_str()) {
                    let has_null = has_null_true(&call.arguments);
                    let has_unique = has_unique_true(&call.arguments);
                    if has_null && !has_unique {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ001",
                                "Avoid null=True on string-based fields. Use blank=True instead (null=True is only needed with unique=True).",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ002: ModelFormUsesExclude ───────────────────────────────────────────

pub struct ModelFormUsesExclude;

impl AstCheck for ModelFormUsesExclude {
    fn code(&self) -> &'static str {
        "DJ002"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = ModelFormExcludeVisitor {
            diags: vec![],
            filename: ctx.filename,
            class_name: String::new(),
            in_modelform: false,
            in_meta: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct ModelFormExcludeVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    class_name: String,
    in_modelform: bool,
    in_meta: bool,
}

impl<'a> Visitor<'_> for ModelFormExcludeVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let was_modelform = self.in_modelform;
            let was_meta = self.in_meta;
            let prev_name = self.class_name.clone();

            if is_model_form(cls) || is_serializer_class(cls) {
                self.in_modelform = true;
                self.in_meta = false;
                self.class_name = cls.name.as_str().to_string();
                visitor::walk_stmt(self, stmt);
                self.in_modelform = was_modelform;
                self.in_meta = was_meta;
                self.class_name = prev_name;
                return;
            }

            if self.in_modelform && cls.name.as_str() == "Meta" {
                self.in_meta = true;
                visitor::walk_stmt(self, stmt);
                self.in_meta = was_meta;
                return;
            }

            visitor::walk_stmt(self, stmt);
        } else if let Stmt::Assign(assign) = stmt {
            if self.in_meta {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "exclude" {
                            let name = self.class_name.clone();
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ002",
                                    format!("ModelForm '{name}' should use 'fields' instead of 'exclude' in Meta."),
                                    self.filename,
                                )
                                .with_range(assign.range()),
                            );
                        }
                    }
                }
            }
        } else {
            visitor::walk_stmt(self, stmt);
        }
    }
}

// ── DJ003: RawSqlUsage ────────────────────────────────────────────────────

pub struct RawSqlUsage;

impl AstCheck for RawSqlUsage {
    fn code(&self) -> &'static str {
        "DJ003"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = RawSqlVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RawSqlVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for RawSqlVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                let method = attr.attr.as_str();
                if method == "raw" || method == "extra" {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ003",
                            "Avoid using .raw()/.extra() — prefer QuerySet methods.",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ004: LocalsInRender ─────────────────────────────────────────────────

pub struct LocalsInRender;

impl AstCheck for LocalsInRender {
    fn code(&self) -> &'static str {
        "DJ004"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = LocalsInRenderVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct LocalsInRenderVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for LocalsInRenderVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let is_render = match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "render",
                Expr::Attribute(a) => a.attr.as_str() == "render",
                _ => false,
            };
            if is_render {
                for arg in &call.arguments.args {
                    if is_locals_call(arg) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ004",
                                "Do not pass locals() as render context — explicitly list variables.",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                        break;
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ005: ModelWithoutStrMethod ──────────────────────────────────────────

pub struct ModelWithoutStrMethod;

impl AstCheck for ModelWithoutStrMethod {
    fn code(&self) -> &'static str {
        "DJ005"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            if let Stmt::ClassDef(cls) = stmt {
                if is_django_model(cls) && !has_abstract_meta(cls) {
                    let has_str = cls.body.iter().any(|s| {
                        if let Stmt::FunctionDef(f) = s {
                            f.name.as_str() == "__str__"
                        } else {
                            false
                        }
                    });
                    if !has_str {
                        let name = cls.name.as_str();
                        diags.push(
                            Diagnostic::new(
                                "DJ005",
                                format!("Model '{name}' is missing a __str__ method."),
                                ctx.filename,
                            )
                            .with_range(cls.range()),
                        );
                    }
                }
            }
        }
        diags
    }
}

// ── DJ006: ForeignKeyMissingOnDelete ─────────────────────────────────────

pub struct ForeignKeyMissingOnDelete;

impl AstCheck for ForeignKeyMissingOnDelete {
    fn code(&self) -> &'static str {
        "DJ006"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = FkOnDeleteVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct FkOnDeleteVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for FkOnDeleteVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let fn_name = match call.func.as_ref() {
                Expr::Name(n) => Some(n.id.as_str().to_string()),
                Expr::Attribute(a) => Some(a.attr.as_str().to_string()),
                _ => None,
            };
            if let Some(name) = fn_name {
                if name == "ForeignKey" || name == "OneToOneField" {
                    let has_on_delete = call
                        .arguments
                        .keywords
                        .iter()
                        .any(|kw| kw.arg.as_ref().is_some_and(|a| a.as_str() == "on_delete"));
                    if !has_on_delete {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ006",
                                "ForeignKey/OneToOneField is missing on_delete argument.",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ007: ModelFormFieldsAll ─────────────────────────────────────────────

pub struct ModelFormFieldsAll;

impl AstCheck for ModelFormFieldsAll {
    fn code(&self) -> &'static str {
        "DJ007"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = ModelFormFieldsAllVisitor {
            diags: vec![],
            filename: ctx.filename,
            class_name: String::new(),
            in_target_class: false,
            in_meta: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct ModelFormFieldsAllVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    class_name: String,
    in_target_class: bool,
    in_meta: bool,
}

impl<'a> Visitor<'_> for ModelFormFieldsAllVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev_target = self.in_target_class;
            let prev_meta = self.in_meta;
            let prev_name = self.class_name.clone();

            if is_model_form(cls) || is_serializer_class(cls) {
                self.in_target_class = true;
                self.in_meta = false;
                self.class_name = cls.name.as_str().to_string();
                visitor::walk_stmt(self, stmt);
                self.in_target_class = prev_target;
                self.in_meta = prev_meta;
                self.class_name = prev_name;
                return;
            }

            if self.in_target_class && cls.name.as_str() == "Meta" {
                self.in_meta = true;
                visitor::walk_stmt(self, stmt);
                self.in_meta = prev_meta;
                return;
            }

            visitor::walk_stmt(self, stmt);
        } else if let Stmt::Assign(assign) = stmt {
            if self.in_meta || self.in_target_class {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "fields" {
                            if let Expr::StringLiteral(s) = assign.value.as_ref() {
                                if s.value.to_str() == "__all__" {
                                    let name = self.class_name.clone();
                                    self.diags.push(
                                        Diagnostic::new(
                                            "DJ007",
                                            format!("'{name}' uses fields = '__all__' — new model fields will be automatically exposed."),
                                            self.filename,
                                        )
                                        .with_range(assign.range()),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        } else {
            visitor::walk_stmt(self, stmt);
        }
    }
}

// ── DJ008: RandomOrderBy ──────────────────────────────────────────────────

pub struct RandomOrderBy;

impl AstCheck for RandomOrderBy {
    fn code(&self) -> &'static str {
        "DJ008"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Seeder/fixture files intentionally use order_by('?') for data variety
        if is_seeder_or_fixture(ctx.filename) {
            return vec![];
        }
        let mut v = RandomOrderByVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RandomOrderByVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for RandomOrderByVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "order_by" {
                    let has_question = call.arguments.args.iter().any(|arg| {
                        if let Expr::StringLiteral(s) = arg {
                            s.value.to_str() == "?"
                        } else {
                            false
                        }
                    });
                    if has_question {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ008",
                                "order_by('?') causes a full table scan with ORDER BY RANDOM().",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ009: QuerysetBoolEval ───────────────────────────────────────────────

pub struct QuerysetBoolEval;

impl AstCheck for QuerysetBoolEval {
    fn code(&self) -> &'static str {
        "DJ009"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = QuerysetBoolEvalVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct QuerysetBoolEvalVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for QuerysetBoolEvalVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::If(if_stmt) = stmt {
            let test = if_stmt.test.as_ref();
            let flagged = if is_queryset_call(test) {
                true
            } else if let Expr::UnaryOp(unary) = test {
                if matches!(unary.op, UnaryOp::Not) {
                    is_queryset_call(unary.operand.as_ref())
                } else {
                    false
                }
            } else {
                false
            };

            if flagged {
                self.diags.push(
                    Diagnostic::new(
                        "DJ009",
                        "QuerySet evaluated in boolean context — loads entire result set. Use .exists() instead.",
                        self.filename,
                    )
                    .with_range(if_stmt.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ010: QuerysetLen ────────────────────────────────────────────────────

pub struct QuerysetLen;

impl AstCheck for QuerysetLen {
    fn code(&self) -> &'static str {
        "DJ010"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = QuerysetLenVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct QuerysetLenVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for QuerysetLenVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Name(n) = call.func.as_ref() {
                if n.id.as_str() == "len" {
                    if let Some(arg) = call.arguments.args.first() {
                        if is_queryset_call(arg) {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ010",
                                    "len() on QuerySet loads all rows into memory. Use .count() for counting.",
                                    self.filename,
                                )
                                .with_range(call.range()),
                            );
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ011: MissingFExpression ─────────────────────────────────────────────

pub struct MissingFExpression;

impl AstCheck for MissingFExpression {
    fn code(&self) -> &'static str {
        "DJ011"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = MissingFExprVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_model_class: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct MissingFExprVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_model_class: bool,
}

impl<'a> Visitor<'_> for MissingFExprVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let was = self.in_model_class;
            if is_django_model(cls) {
                self.in_model_class = true;
            }
            visitor::walk_stmt(self, stmt);
            self.in_model_class = was;
            return;
        }

        if self.in_model_class {
            // Detect: self.field += N (AugAssign)
            if let Stmt::AugAssign(aug) = stmt {
                if let Expr::Attribute(attr) = aug.target.as_ref() {
                    if is_self_access(attr.value.as_ref()) {
                        let field_name = attr.attr.as_str().to_string();
                        self.diags.push(
                            Diagnostic::new(
                                "DJ011",
                                format!("'self.{field_name} += ...' is a race condition under concurrency. Use F() for atomic updates."),
                                self.filename,
                            )
                            .with_range(aug.range()),
                        );
                    }
                }
            }

            // Detect: self.field = self.field + N (Assign where value is BinOp with self.field)
            if let Stmt::Assign(assign) = stmt {
                if let Expr::BinOp(binop) = assign.value.as_ref() {
                    if let Expr::Attribute(left_attr) = binop.left.as_ref() {
                        if is_self_access(left_attr.value.as_ref()) {
                            // Check target is also self.field
                            for target in &assign.targets {
                                if let Expr::Attribute(t_attr) = target {
                                    if is_self_access(t_attr.value.as_ref()) {
                                        let field_name = t_attr.attr.as_str().to_string();
                                        self.diags.push(
                                            Diagnostic::new(
                                                "DJ011",
                                                format!("'self.{field_name} += ...' is a race condition under concurrency. Use F() for atomic updates."),
                                                self.filename,
                                            )
                                            .with_range(assign.range()),
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ014: RawSqlInjection ────────────────────────────────────────────────

pub struct RawSqlInjection;

impl AstCheck for RawSqlInjection {
    fn code(&self) -> &'static str {
        "DJ014"
    }

    // Downgraded from High: f-string SQL also catches safe constant interpolation.
    // A future version should track whether interpolated variables are module-level constants.
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Improve
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Skip migration files — SQL in migrations is developer-written, not user input
        if ctx.filename.contains("/migrations/") {
            return vec![];
        }
        let mut v = RawSqlInjectionVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RawSqlInjectionVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

fn is_interpolated_string(expr: &Expr) -> bool {
    match expr {
        Expr::FString(_) => true,
        Expr::Call(call) => {
            // .format() call
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "format" {
                    return true;
                }
            }
            false
        }
        Expr::BinOp(binop) => {
            // % operator: "sql %s" % values
            matches!(binop.op, Operator::Mod)
        }
        _ => false,
    }
}

impl<'a> Visitor<'_> for RawSqlInjectionVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                let method = attr.attr.as_str();
                if matches!(method, "raw" | "execute" | "extra") {
                    if let Some(first_arg) = call.arguments.args.first() {
                        if is_interpolated_string(first_arg) {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ014",
                                    format!(".{method}() with string interpolation is a SQL injection risk."),
                                    self.filename,
                                )
                                .with_range(call.range()),
                            );
                        }
                    }
                    // Also check keyword args
                    for kw in &call.arguments.keywords {
                        if kw.arg.as_ref().is_some_and(|a| a.as_str() == "sql")
                            && is_interpolated_string(&kw.value)
                        {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ014",
                                    format!(".{method}() with string interpolation is a SQL injection risk."),
                                    self.filename,
                                )
                                .with_range(call.range()),
                            );
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ015: DefaultMetaOrdering ────────────────────────────────────────────

pub struct DefaultMetaOrdering;

impl AstCheck for DefaultMetaOrdering {
    fn code(&self) -> &'static str {
        "DJ015"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = DefaultMetaOrderingVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_model: false,
            model_name: String::new(),
            in_meta: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DefaultMetaOrderingVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_model: bool,
    model_name: String,
    in_meta: bool,
}

impl<'a> Visitor<'_> for DefaultMetaOrderingVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev_model = self.in_model;
            let prev_meta = self.in_meta;
            let prev_name = self.model_name.clone();

            if is_django_model(cls) {
                self.in_model = true;
                self.in_meta = false;
                self.model_name = cls.name.as_str().to_string();
                visitor::walk_stmt(self, stmt);
                self.in_model = prev_model;
                self.in_meta = prev_meta;
                self.model_name = prev_name;
                return;
            }

            if self.in_model && cls.name.as_str() == "Meta" {
                self.in_meta = true;
                visitor::walk_stmt(self, stmt);
                self.in_meta = prev_meta;
                return;
            }

            visitor::walk_stmt(self, stmt);
        } else if let Stmt::Assign(assign) = stmt {
            if self.in_meta {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "ordering" {
                            let name = self.model_name.clone();
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ015",
                                    format!("Model '{name}' has default Meta.ordering — adds ORDER BY to every query."),
                                    self.filename,
                                )
                                .with_range(assign.range()),
                            );
                        }
                    }
                }
            }
        } else {
            visitor::walk_stmt(self, stmt);
        }
    }
}

// ── DJ017: CsrfExempt ────────────────────────────────────────────────────

pub struct CsrfExempt;

impl AstCheck for CsrfExempt {
    fn code(&self) -> &'static str {
        "DJ017"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Skip if the filename itself is a webhook/callback/api file
        let fname_lower = ctx.filename.to_lowercase();
        let skip_file = ["webhook", "callback", "hook", "api"]
            .iter()
            .any(|kw| fname_lower.contains(kw));
        if skip_file {
            return vec![];
        }

        let mut v = CsrfExemptVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct CsrfExemptVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for CsrfExemptVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(func) = stmt {
            let fn_name_lower = func.name.as_str().to_lowercase();
            let skip_fn = ["webhook", "callback", "hook", "api"]
                .iter()
                .any(|kw| fn_name_lower.contains(kw));

            if !skip_fn {
                let has_csrf_exempt = func.decorator_list.iter().any(|dec| match &dec.expression {
                    Expr::Name(n) => n.id.as_str() == "csrf_exempt",
                    Expr::Attribute(a) => a.attr.as_str() == "csrf_exempt",
                    _ => false,
                });
                if has_csrf_exempt {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ017",
                            "@csrf_exempt disables CSRF protection on a non-webhook view.",
                            self.filename,
                        )
                        .with_range(func.range()),
                    );
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ018: RequestPostBoolCheck ───────────────────────────────────────────

pub struct RequestPostBoolCheck;

impl AstCheck for RequestPostBoolCheck {
    fn code(&self) -> &'static str {
        "DJ018"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = RequestPostBoolVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RequestPostBoolVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

fn is_request_post(expr: &Expr) -> bool {
    if let Expr::Attribute(attr) = expr {
        if attr.attr.as_str() == "POST" {
            if let Expr::Name(n) = attr.value.as_ref() {
                return n.id.as_str() == "request";
            }
        }
    }
    false
}

impl<'a> Visitor<'_> for RequestPostBoolVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::If(if_stmt) = stmt {
            let test = if_stmt.test.as_ref();
            let flagged = if is_request_post(test) {
                true
            } else if let Expr::UnaryOp(unary) = test {
                matches!(unary.op, UnaryOp::Not) && is_request_post(unary.operand.as_ref())
            } else {
                false
            };
            if flagged {
                self.diags.push(
                    Diagnostic::new(
                        "DJ018",
                        "'if request.POST' is falsy for empty POST bodies. Use 'if request.method == \"POST\"'.",
                        self.filename,
                    )
                    .with_range(if_stmt.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ019: CountGreaterThanZero ───────────────────────────────────────────

pub struct CountGreaterThanZero;

impl AstCheck for CountGreaterThanZero {
    fn code(&self) -> &'static str {
        "DJ019"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = CountGtZeroVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct CountGtZeroVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

fn is_count_call(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        if let Expr::Attribute(attr) = call.func.as_ref() {
            return attr.attr.as_str() == "count";
        }
    }
    false
}

fn is_zero_literal(expr: &Expr) -> bool {
    if let Expr::NumberLiteral(n) = expr {
        if let Number::Int(i) = &n.value {
            return i.as_u8() == Some(0);
        }
    }
    false
}

impl<'a> Visitor<'_> for CountGtZeroVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Compare(cmp) = expr {
            let left_is_count = is_count_call(cmp.left.as_ref());
            let right_is_count = cmp.comparators.first().is_some_and(is_count_call);

            let flagged = if left_is_count {
                cmp.comparators.first().is_some_and(is_zero_literal)
                    && cmp
                        .ops
                        .first()
                        .is_some_and(|op| matches!(op, CmpOp::Gt | CmpOp::NotEq | CmpOp::Eq))
            } else if right_is_count {
                is_zero_literal(cmp.left.as_ref())
                    && cmp
                        .ops
                        .first()
                        .is_some_and(|op| matches!(op, CmpOp::Lt | CmpOp::NotEq | CmpOp::Eq))
            } else {
                false
            };

            if flagged {
                self.diags.push(
                    Diagnostic::new(
                        "DJ019",
                        ".count() > 0 scans all rows. Use .exists() instead.",
                        self.filename,
                    )
                    .with_range(cmp.range()),
                );
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ020: SelectRelatedNoArgs ────────────────────────────────────────────

pub struct SelectRelatedNoArgs;

impl AstCheck for SelectRelatedNoArgs {
    fn code(&self) -> &'static str {
        "DJ020"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = SelectRelatedNoArgsVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SelectRelatedNoArgsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for SelectRelatedNoArgsVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "select_related"
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty()
                {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ020",
                            "select_related() without arguments follows ALL FK chains. Specify fields explicitly.",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ021: FloatFieldForMoney ─────────────────────────────────────────────

pub struct FloatFieldForMoney;

impl AstCheck for FloatFieldForMoney {
    fn code(&self) -> &'static str {
        "DJ021"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = FloatFieldForMoneyVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_model: false,
            current_assign_target: None,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct FloatFieldForMoneyVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_model: bool,
    current_assign_target: Option<String>,
}

const MONEY_KEYWORDS: &[&str] = &[
    "price", "cost", "amount", "fee", "total", "balance", "salary", "payment", "money", "currency",
    "rate",
];

fn target_name_is_money(name: &str) -> bool {
    let lower = name.to_lowercase();
    MONEY_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

impl<'a> Visitor<'_> for FloatFieldForMoneyVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let was = self.in_model;
            if is_django_model(cls) {
                self.in_model = true;
            }
            visitor::walk_stmt(self, stmt);
            self.in_model = was;
            return;
        }

        if self.in_model {
            if let Stmt::Assign(assign) = stmt {
                // Get the target name
                let target_name = assign.targets.first().and_then(|t| {
                    if let Expr::Name(n) = t {
                        Some(n.id.as_str().to_string())
                    } else {
                        None
                    }
                });
                let prev = self.current_assign_target.clone();
                self.current_assign_target = target_name;
                visitor::walk_stmt(self, stmt);
                self.current_assign_target = prev;
                return;
            }
        }

        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if self.in_model {
            if let Expr::Call(call) = expr {
                let fn_name = match call.func.as_ref() {
                    Expr::Name(n) => Some(n.id.as_str().to_string()),
                    Expr::Attribute(a) => Some(a.attr.as_str().to_string()),
                    _ => None,
                };
                if fn_name.as_deref() == Some("FloatField") {
                    let is_money = self
                        .current_assign_target
                        .as_deref()
                        .is_some_and(target_name_is_money);
                    if is_money {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ021",
                                "FloatField causes rounding errors. Use DecimalField for currency/money.",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ022: MutableDefaultJsonField ───────────────────────────────────────

pub struct MutableDefaultJsonField;

impl AstCheck for MutableDefaultJsonField {
    fn code(&self) -> &'static str {
        "DJ022"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = MutableDefaultJsonVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct MutableDefaultJsonVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for MutableDefaultJsonVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let fn_name = match call.func.as_ref() {
                Expr::Name(n) => Some(n.id.as_str().to_string()),
                Expr::Attribute(a) => Some(a.attr.as_str().to_string()),
                _ => None,
            };
            if let Some(name) = &fn_name {
                if matches!(name.as_str(), "JSONField" | "ArrayField") {
                    for kw in &call.arguments.keywords {
                        if kw.arg.as_ref().is_some_and(|a| a.as_str() == "default") {
                            let is_mutable = matches!(&kw.value, Expr::List(_) | Expr::Dict(_));
                            if is_mutable {
                                self.diags.push(
                                    Diagnostic::new(
                                        "DJ022",
                                        "Mutable default on JSONField/ArrayField is shared across instances. Use default=dict or default=list.",
                                        self.filename,
                                    )
                                    .with_range(call.range()),
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

// ── DJ023: SignalWithoutDispatchUid ───────────────────────────────────────

pub struct SignalWithoutDispatchUid;

impl AstCheck for SignalWithoutDispatchUid {
    fn code(&self) -> &'static str {
        "DJ023"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = SignalDispatchUidVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SignalDispatchUidVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for SignalDispatchUidVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        // @receiver(...) decorator on a function — check for dispatch_uid
        if let Stmt::FunctionDef(func) = stmt {
            for dec in &func.decorator_list {
                if let Expr::Call(call) = &dec.expression {
                    let is_receiver = match call.func.as_ref() {
                        Expr::Name(n) => n.id.as_str() == "receiver",
                        Expr::Attribute(a) => a.attr.as_str() == "receiver",
                        _ => false,
                    };
                    if is_receiver {
                        let has_uid = call.arguments.keywords.iter().any(|kw| {
                            kw.arg
                                .as_ref()
                                .is_some_and(|a| a.as_str() == "dispatch_uid")
                        });
                        if !has_uid {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ023",
                                    "Signal receiver without dispatch_uid may fire multiple times if module is re-imported.",
                                    self.filename,
                                )
                                .with_range(func.range()),
                            );
                        }
                    }
                }
            }
        }

        // signal.connect(handler) without dispatch_uid
        if let Stmt::Expr(expr_stmt) = stmt {
            if let Expr::Call(call) = expr_stmt.value.as_ref() {
                if let Expr::Attribute(attr) = call.func.as_ref() {
                    if attr.attr.as_str() == "connect" {
                        let has_uid = call.arguments.keywords.iter().any(|kw| {
                            kw.arg
                                .as_ref()
                                .is_some_and(|a| a.as_str() == "dispatch_uid")
                        });
                        if !has_uid {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ023",
                                    "Signal receiver without dispatch_uid may fire multiple times if module is re-imported.",
                                    self.filename,
                                )
                                .with_range(call.range()),
                            );
                        }
                    }
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ024: UniqueTogetherDeprecated ──────────────────────────────────────

pub struct UniqueTogetherDeprecated;

impl AstCheck for UniqueTogetherDeprecated {
    fn code(&self) -> &'static str {
        "DJ024"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = MetaAttrVisitor {
            diags: vec![],
            filename: ctx.filename,
            code: "DJ024",
            message:
                "unique_together is deprecated. Use Meta.constraints with UniqueConstraint instead.",
            attr_name: "unique_together",
            in_model: false,
            model_name: String::new(),
            in_meta: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

// ── DJ025: IndexTogetherDeprecated ────────────────────────────────────────

pub struct IndexTogetherDeprecated;

impl AstCheck for IndexTogetherDeprecated {
    fn code(&self) -> &'static str {
        "DJ025"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = MetaAttrVisitor {
            diags: vec![],
            filename: ctx.filename,
            code: "DJ025",
            message: "index_together is removed in Django 5.1. Use Meta.indexes with models.Index instead.",
            attr_name: "index_together",
            in_model: false,
            model_name: String::new(),
            in_meta: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

/// Shared visitor for checks that detect a specific attribute name in a Model's Meta class.
struct MetaAttrVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    code: &'static str,
    message: &'static str,
    attr_name: &'static str,
    in_model: bool,
    model_name: String,
    in_meta: bool,
}

impl<'a> Visitor<'_> for MetaAttrVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev_model = self.in_model;
            let prev_meta = self.in_meta;
            let prev_name = self.model_name.clone();

            if is_django_model(cls) {
                self.in_model = true;
                self.in_meta = false;
                self.model_name = cls.name.as_str().to_string();
                visitor::walk_stmt(self, stmt);
                self.in_model = prev_model;
                self.in_meta = prev_meta;
                self.model_name = prev_name;
                return;
            }

            if self.in_model && cls.name.as_str() == "Meta" {
                self.in_meta = true;
                visitor::walk_stmt(self, stmt);
                self.in_meta = prev_meta;
                return;
            }

            visitor::walk_stmt(self, stmt);
        } else if let Stmt::Assign(assign) = stmt {
            if self.in_meta {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == self.attr_name {
                            self.diags.push(
                                Diagnostic::new(self.code, self.message, self.filename)
                                    .with_range(assign.range()),
                            );
                        }
                    }
                }
            }
        } else {
            visitor::walk_stmt(self, stmt);
        }
    }
}

// ── DJ026: SaveCreateInLoop ───────────────────────────────────────────────

pub struct SaveCreateInLoop;

impl AstCheck for SaveCreateInLoop {
    fn code(&self) -> &'static str {
        "DJ026"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.filename.contains("/tests/") || ctx.filename.contains("/migrations/") {
            return vec![];
        }
        // Seeder/fixture files intentionally create records in loops
        if is_seeder_or_fixture(ctx.filename) {
            return vec![];
        }
        let mut v = SaveCreateInLoopVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SaveCreateInLoopVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for SaveCreateInLoopVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::For(for_stmt) = stmt {
            let mut finder = LoopDbCallFinder {
                diags: vec![],
                filename: self.filename,
                in_try: false,
                in_early_exit_if: false,
            };
            for body_stmt in &for_stmt.body {
                finder.visit_stmt(body_stmt);
            }
            self.diags.extend(finder.diags);
        }
        // Also check while loops
        if let Stmt::While(while_stmt) = stmt {
            let mut finder = LoopDbCallFinder {
                diags: vec![],
                filename: self.filename,
                in_try: false,
                in_early_exit_if: false,
            };
            for body_stmt in &while_stmt.body {
                finder.visit_stmt(body_stmt);
            }
            self.diags.extend(finder.diags);
        }
        visitor::walk_stmt(self, stmt);
    }
}

struct LoopDbCallFinder<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_try: bool,
    /// True when we are inside an if-block that ends with return/break. Saves
    /// inside such blocks execute at most once per loop iteration and should not
    /// be flagged (search-and-update / early-exit pattern).
    in_early_exit_if: bool,
}

/// Returns true if any statement in `body` is a `return` or `break` at the
/// top level, indicating the block exits the loop after executing.
fn block_has_early_exit(body: &[Stmt]) -> bool {
    body.iter()
        .any(|s| matches!(s, Stmt::Return(_) | Stmt::Break(_)))
}

impl<'a> Visitor<'_> for LoopDbCallFinder<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        // Track try/except blocks — saves inside try are error-handling patterns.
        if let Stmt::Try(try_stmt) = stmt {
            let prev = self.in_try;
            self.in_try = true;
            for s in &try_stmt.body {
                visitor::walk_stmt(self, s);
            }
            self.in_try = prev;
            // still walk handlers/else/finally
            for handler in &try_stmt.handlers {
                let ExceptHandler::ExceptHandler(h) = handler;
                for s in &h.body {
                    visitor::walk_stmt(self, s);
                }
            }
            for s in &try_stmt.orelse {
                visitor::walk_stmt(self, s);
            }
            for s in &try_stmt.finalbody {
                visitor::walk_stmt(self, s);
            }
            return;
        }

        // Don't descend into nested for/while loops here (outer visitor handles those)
        if matches!(stmt, Stmt::For(_) | Stmt::While(_)) {
            return;
        }

        // For `if` blocks: if the body terminates with return/break, any save
        // inside it only runs once and the loop exits — not a bulk-write pattern.
        if let Stmt::If(if_stmt) = stmt {
            // Walk the if-body, marking early-exit status.
            let prev = self.in_early_exit_if;
            self.in_early_exit_if = block_has_early_exit(&if_stmt.body);
            for s in &if_stmt.body {
                visitor::walk_stmt(self, s);
            }
            // Walk each elif/else clause with its own early-exit status.
            for clause in &if_stmt.elif_else_clauses {
                self.in_early_exit_if = block_has_early_exit(&clause.body);
                for s in &clause.body {
                    visitor::walk_stmt(self, s);
                }
            }
            self.in_early_exit_if = prev;
            return;
        }

        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if self.in_try || self.in_early_exit_if {
            visitor::walk_expr(self, expr);
            return;
        }

        if let Expr::Call(call) = expr {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                let method = attr.attr.as_str();
                if method == "save" {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ026",
                            ".save()/.create() in a loop executes N queries. Use bulk_create()/bulk_update() instead.",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
                if method == "create" {
                    // Check it's objects.create()
                    if let Expr::Attribute(inner) = attr.value.as_ref() {
                        if inner.attr.as_str() == "objects" {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ026",
                                    ".save()/.create() in a loop executes N queries. Use bulk_create()/bulk_update() instead.",
                                    self.filename,
                                )
                                .with_range(call.range()),
                            );
                        }
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ027: CeleryDelayInAtomic ────────────────────────────────────────────

pub struct CeleryDelayInAtomic;

impl AstCheck for CeleryDelayInAtomic {
    fn code(&self) -> &'static str {
        "DJ027"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = CeleryDelayInAtomicVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_atomic: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct CeleryDelayInAtomicVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_atomic: bool,
}

fn is_transaction_atomic(expr: &Expr) -> bool {
    match expr {
        Expr::Attribute(a) => {
            a.attr.as_str() == "atomic"
                && matches!(a.value.as_ref(), Expr::Attribute(inner) if inner.attr.as_str() == "transaction"
                    || matches!(inner.value.as_ref(), Expr::Name(n) if n.id.as_str() == "transaction"))
                || matches!(a.value.as_ref(), Expr::Name(n) if n.id.as_str() == "transaction" && a.attr.as_str() == "atomic")
        }
        Expr::Call(call) => is_transaction_atomic(call.func.as_ref()),
        Expr::Name(n) => n.id.as_str() == "atomic",
        _ => false,
    }
}

impl<'a> Visitor<'_> for CeleryDelayInAtomicVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::With(with_stmt) = stmt {
            for item in &with_stmt.items {
                if is_transaction_atomic(&item.context_expr) {
                    let prev = self.in_atomic;
                    self.in_atomic = true;
                    for s in &with_stmt.body {
                        visitor::walk_stmt(self, s);
                    }
                    self.in_atomic = prev;
                    return;
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &Expr) {
        if self.in_atomic {
            if let Expr::Call(call) = expr {
                if let Expr::Attribute(attr) = call.func.as_ref() {
                    let method = attr.attr.as_str();
                    if method == "delay" || method == "apply_async" {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ027",
                                "Celery task dispatched inside transaction.atomic() may execute before commit. Use transaction.on_commit().",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ028: RedirectReverse ────────────────────────────────────────────────

pub struct RedirectReverse;

impl AstCheck for RedirectReverse {
    fn code(&self) -> &'static str {
        "DJ028"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = RedirectReverseVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RedirectReverseVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

fn is_reverse_call(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        return match call.func.as_ref() {
            Expr::Name(n) => n.id.as_str() == "reverse",
            Expr::Attribute(a) => a.attr.as_str() == "reverse",
            _ => false,
        };
    }
    false
}

impl<'a> Visitor<'_> for RedirectReverseVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let is_redirect = match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "redirect",
                Expr::Attribute(a) => a.attr.as_str() == "redirect",
                _ => false,
            };
            if is_redirect {
                if let Some(first_arg) = call.arguments.args.first() {
                    if is_reverse_call(first_arg) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ028",
                                "redirect(reverse('name')) is redundant. Use redirect('name') directly.",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ029: UnfilteredDelete ───────────────────────────────────────────────

pub struct UnfilteredDelete;

impl AstCheck for UnfilteredDelete {
    fn code(&self) -> &'static str {
        "DJ029"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Improve
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.filename.contains("/tests/") {
            return vec![];
        }
        let mut v = UnfilteredDeleteVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct UnfilteredDeleteVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

/// Check for: Model.objects.all().delete() or Model.objects.delete()
fn is_unfiltered_delete_chain(call: &ExprCall) -> bool {
    if let Expr::Attribute(attr) = call.func.as_ref() {
        if attr.attr.as_str() != "delete" {
            return false;
        }
        // .objects.all().delete() — receiver is .all() call
        if let Expr::Call(inner_call) = attr.value.as_ref() {
            if let Expr::Attribute(inner_attr) = inner_call.func.as_ref() {
                if inner_attr.attr.as_str() == "all" {
                    // Check .objects.all()
                    if let Expr::Attribute(obj_attr) = inner_attr.value.as_ref() {
                        if obj_attr.attr.as_str() == "objects" {
                            return true;
                        }
                    }
                }
            }
        }
        // .objects.delete() directly
        if let Expr::Attribute(obj_attr) = attr.value.as_ref() {
            if obj_attr.attr.as_str() == "objects" {
                return true;
            }
        }
    }
    false
}

impl<'a> Visitor<'_> for UnfilteredDeleteVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if is_unfiltered_delete_chain(call) {
                self.diags.push(
                    Diagnostic::new(
                        "DJ029",
                        "Unfiltered .delete() removes ALL rows. Add .filter() if this is intentional.",
                        self.filename,
                    )
                    .with_range(call.range()),
                );
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── DJ030: DRFAllowAnyPermission ──────────────────────────────────────────

pub struct DRFAllowAnyPermission;

impl AstCheck for DRFAllowAnyPermission {
    fn code(&self) -> &'static str {
        "DJ030"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = DRFPermissionVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_drf_view: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DRFPermissionVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_drf_view: bool,
}

impl<'a> Visitor<'_> for DRFPermissionVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev = self.in_drf_view;
            if is_drf_view(cls) {
                self.in_drf_view = true;
                visitor::walk_stmt(self, stmt);
                self.in_drf_view = prev;
                return;
            }
            visitor::walk_stmt(self, stmt);
            return;
        }

        if self.in_drf_view {
            if let Stmt::Assign(assign) = stmt {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "permission_classes" {
                            let is_empty_or_allow_any = match assign.value.as_ref() {
                                Expr::List(lst) => {
                                    lst.elts.is_empty()
                                        || lst.elts.iter().any(|e| match e {
                                            Expr::Name(n) => n.id.as_str() == "AllowAny",
                                            Expr::Attribute(a) => a.attr.as_str() == "AllowAny",
                                            _ => false,
                                        })
                                }
                                Expr::Tuple(tup) => {
                                    tup.elts.is_empty()
                                        || tup.elts.iter().any(|e| match e {
                                            Expr::Name(n) => n.id.as_str() == "AllowAny",
                                            Expr::Attribute(a) => a.attr.as_str() == "AllowAny",
                                            _ => false,
                                        })
                                }
                                _ => false,
                            };
                            if is_empty_or_allow_any {
                                self.diags.push(
                                    Diagnostic::new(
                                        "DJ030",
                                        "View has AllowAny/empty permissions — any user can access this endpoint.",
                                        self.filename,
                                    )
                                    .with_range(assign.range()),
                                );
                            }
                        }
                    }
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ031: DRFEmptyAuthClasses ────────────────────────────────────────────

pub struct DRFEmptyAuthClasses;

impl AstCheck for DRFEmptyAuthClasses {
    fn code(&self) -> &'static str {
        "DJ031"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = DRFEmptyAuthVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_drf_view: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DRFEmptyAuthVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_drf_view: bool,
}

impl<'a> Visitor<'_> for DRFEmptyAuthVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev = self.in_drf_view;
            if is_drf_view(cls) {
                self.in_drf_view = true;
                visitor::walk_stmt(self, stmt);
                self.in_drf_view = prev;
                return;
            }
            visitor::walk_stmt(self, stmt);
            return;
        }

        if self.in_drf_view {
            if let Stmt::Assign(assign) = stmt {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "authentication_classes" {
                            let is_empty = match assign.value.as_ref() {
                                Expr::List(lst) => lst.elts.is_empty(),
                                Expr::Tuple(tup) => tup.elts.is_empty(),
                                _ => false,
                            };
                            if is_empty {
                                self.diags.push(
                                    Diagnostic::new(
                                        "DJ031",
                                        "Empty authentication_classes disables authentication for this view.",
                                        self.filename,
                                    )
                                    .with_range(assign.range()),
                                );
                            }
                        }
                    }
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ032: DjangoValidationErrorInDRF ────────────────────────────────────

pub struct DjangoValidationErrorInDRF;

impl AstCheck for DjangoValidationErrorInDRF {
    fn code(&self) -> &'static str {
        "DJ032"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Skip migration files
        if ctx.filename.contains("/migrations/") {
            return vec![];
        }

        // Determine the local name under which Django's ValidationError was imported,
        // and whether this file also imports from rest_framework.
        let mut django_ve_local_name: Option<String> = None;
        let mut import_range = None;
        let mut has_rest_framework = false;

        for stmt in &ctx.module.body {
            if let Stmt::ImportFrom(imp) = stmt {
                let module = imp.module.as_ref().map(|m| m.as_str()).unwrap_or("");
                if module == "django.core.exceptions" {
                    for alias in &imp.names {
                        if alias.name.as_str() == "ValidationError" {
                            // Respect `as` aliases: `from ... import ValidationError as DVE`
                            let local = alias
                                .asname
                                .as_ref()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|| "ValidationError".to_string());
                            django_ve_local_name = Some(local);
                            import_range = Some(imp.range());
                        }
                    }
                }
                if module.starts_with("rest_framework") {
                    has_rest_framework = true;
                }
            }
        }

        let (Some(local_name), true, Some(range)) =
            (django_ve_local_name, has_rest_framework, import_range)
        else {
            return vec![];
        };

        // Only flag when Django's ValidationError is actually *raised*. Catching it
        // in an `except ValidationError` handler is correct DRF usage and must not
        // be flagged.
        if is_django_ve_raised(&ctx.module.body, &local_name) {
            return vec![Diagnostic::new(
                "DJ032",
                "Using Django's ValidationError in DRF code causes 500 errors. Use rest_framework.exceptions.ValidationError.",
                ctx.filename,
            )
            .with_range(range)];
        }

        vec![]
    }
}

/// Recursively walk a list of statements to detect `raise ValidationError(...)`
/// (identified by its local import name). Returns true as soon as one is found.
/// Ignores uses inside `except ValidationError` handlers — those are catching it,
/// not raising it, so they are safe.
fn is_django_ve_raised(body: &[Stmt], local_name: &str) -> bool {
    for stmt in body {
        if stmt_raises_django_ve(stmt, local_name) {
            return true;
        }
    }
    false
}

fn stmt_raises_django_ve(stmt: &Stmt, local_name: &str) -> bool {
    match stmt {
        Stmt::Raise(raise_stmt) => {
            // `raise ValidationError(...)` or `raise ValidationError`
            if let Some(exc) = &raise_stmt.exc {
                return expr_names_django_ve(exc, local_name);
            }
        }
        Stmt::FunctionDef(func) => {
            if is_django_ve_raised(&func.body, local_name) {
                return true;
            }
        }
        Stmt::ClassDef(cls) => {
            if is_django_ve_raised(&cls.body, local_name) {
                return true;
            }
        }
        Stmt::If(if_stmt) => {
            if is_django_ve_raised(&if_stmt.body, local_name) {
                return true;
            }
            for clause in &if_stmt.elif_else_clauses {
                if is_django_ve_raised(&clause.body, local_name) {
                    return true;
                }
            }
        }
        Stmt::For(for_stmt) => {
            if is_django_ve_raised(&for_stmt.body, local_name)
                || is_django_ve_raised(&for_stmt.orelse, local_name)
            {
                return true;
            }
        }
        Stmt::While(while_stmt) => {
            if is_django_ve_raised(&while_stmt.body, local_name)
                || is_django_ve_raised(&while_stmt.orelse, local_name)
            {
                return true;
            }
        }
        Stmt::With(with_stmt) => {
            if is_django_ve_raised(&with_stmt.body, local_name) {
                return true;
            }
        }
        Stmt::Try(try_stmt) => {
            // Check the try body and else/finally blocks for raises.
            // Do NOT recurse into except handlers — catching the error is fine.
            if is_django_ve_raised(&try_stmt.body, local_name)
                || is_django_ve_raised(&try_stmt.orelse, local_name)
                || is_django_ve_raised(&try_stmt.finalbody, local_name)
            {
                return true;
            }
        }
        _ => {}
    }
    false
}

/// Returns true if this expression is `ValidationError` (bare name) or
/// `ValidationError(...)` (a call to it).
fn expr_names_django_ve(expr: &Expr, local_name: &str) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == local_name,
        Expr::Call(call) => expr_names_django_ve(&call.func, local_name),
        _ => false,
    }
}

// ── DJ033: DRFNoPaginationClass ───────────────────────────────────────────

pub struct DRFNoPaginationClass;

impl AstCheck for DRFNoPaginationClass {
    fn code(&self) -> &'static str {
        "DJ033"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();

        for stmt in &ctx.module.body {
            if let Stmt::ClassDef(cls) = stmt {
                if is_list_view_class(cls) {
                    // Only flag concrete views that actually bind a queryset — base/mixin
                    // classes without `queryset = ...` are intentionally left abstract.
                    let has_queryset = cls.body.iter().any(|s| {
                        if let Stmt::Assign(assign) = s {
                            assign.targets.iter().any(|t| {
                                if let Expr::Name(n) = t {
                                    n.id.as_str() == "queryset"
                                } else {
                                    false
                                }
                            })
                        } else {
                            false
                        }
                    });
                    if !has_queryset {
                        // No queryset binding — treat as a base/mixin class, skip.
                        continue;
                    }

                    let has_pagination = cls.body.iter().any(|s| {
                        if let Stmt::Assign(assign) = s {
                            assign.targets.iter().any(|t| {
                                if let Expr::Name(n) = t {
                                    n.id.as_str() == "pagination_class"
                                } else {
                                    false
                                }
                            })
                        } else {
                            false
                        }
                    });
                    if !has_pagination {
                        let name = cls.name.as_str();
                        diags.push(
                            Diagnostic::new(
                                "DJ033",
                                "List view without pagination_class returns ALL objects."
                                    .to_string(),
                                ctx.filename,
                            )
                            .with_range(cls.range()),
                        );
                        let _ = name;
                    }
                }
            }
        }
        diags
    }
}

fn is_list_view_class(cls: &StmtClassDef) -> bool {
    let list_view_bases = &[
        "ModelViewSet",
        "ListAPIView",
        "ListModelMixin",
        "ReadOnlyModelViewSet",
    ];
    cls.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| {
            let name = match base {
                Expr::Name(n) => Some(n.id.as_str()),
                Expr::Attribute(a) => Some(a.attr.as_str()),
                _ => None,
            };
            name.is_some_and(|n| list_view_bases.contains(&n))
        })
    })
}

// ── E5101: ModelUnicodeNotCallable ────────────────────────────────────────

pub struct ModelUnicodeNotCallable;

impl AstCheck for ModelUnicodeNotCallable {
    fn code(&self) -> &'static str {
        "E5101"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = ModelUnicodeNotCallableVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_model: false,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct ModelUnicodeNotCallableVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_model: bool,
}

impl<'a> Visitor<'_> for ModelUnicodeNotCallableVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev = self.in_model;
            if is_django_model(cls) {
                self.in_model = true;
            }
            visitor::walk_stmt(self, stmt);
            self.in_model = prev;
            return;
        }

        if self.in_model {
            if let Stmt::Assign(assign) = stmt {
                for target in &assign.targets {
                    if let Expr::Name(n) = target {
                        if n.id.as_str() == "__unicode__" {
                            let is_function = matches!(assign.value.as_ref(), Expr::Lambda(_))
                                || matches!(assign.value.as_ref(), Expr::Name(_));
                            // It's non-callable if it's a literal (string, number, etc.)
                            let is_non_callable = matches!(
                                assign.value.as_ref(),
                                Expr::StringLiteral(_)
                                    | Expr::NumberLiteral(_)
                                    | Expr::BooleanLiteral(_)
                                    | Expr::NoneLiteral(_)
                                    | Expr::List(_)
                                    | Expr::Dict(_)
                            );
                            let _ = is_function;
                            if is_non_callable {
                                self.diags.push(
                                    Diagnostic::new(
                                        "E5101",
                                        "__unicode__ on model must be callable.",
                                        self.filename,
                                    )
                                    .with_range(assign.range()),
                                );
                            }
                        }
                    }
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── W5102: ModelHasUnicode ────────────────────────────────────────────────

pub struct ModelHasUnicode;

impl AstCheck for ModelHasUnicode {
    fn code(&self) -> &'static str {
        "W5102"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = ModelHasUnicodeVisitor {
            diags: vec![],
            filename: ctx.filename,
            in_model: false,
            model_name: String::new(),
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct ModelHasUnicodeVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    in_model: bool,
    model_name: String,
}

impl<'a> Visitor<'_> for ModelHasUnicodeVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::ClassDef(cls) = stmt {
            let prev = self.in_model;
            let prev_name = self.model_name.clone();
            if is_django_model(cls) {
                self.in_model = true;
                self.model_name = cls.name.as_str().to_string();
            }
            visitor::walk_stmt(self, stmt);
            self.in_model = prev;
            self.model_name = prev_name;
            return;
        }

        if self.in_model {
            if let Stmt::FunctionDef(func) = stmt {
                if func.name.as_str() == "__unicode__" {
                    let name = self.model_name.clone();
                    self.diags.push(
                        Diagnostic::new(
                            "W5102",
                            format!("Model '{name}' defines __unicode__ — Python 3 uses __str__ instead."),
                            self.filename,
                        )
                        .with_range(func.range()),
                    );
                }
            }
        }

        visitor::walk_stmt(self, stmt);
    }
}

// ── E5141: HardCodedAuthUser ──────────────────────────────────────────────

pub struct HardCodedAuthUser;

impl AstCheck for HardCodedAuthUser {
    fn code(&self) -> &'static str {
        "E5141"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = HardCodedAuthUserVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct HardCodedAuthUserVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for HardCodedAuthUserVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::StringLiteral(s) = expr {
            if s.value.to_str() == "auth.User" {
                self.diags.push(
                    Diagnostic::new(
                        "E5141",
                        "Hard-coded reference to 'auth.User' — use settings.AUTH_USER_MODEL instead.",
                        self.filename,
                    )
                    .with_range(s.range()),
                );
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── E5142: ImportedAuthUser ───────────────────────────────────────────────

pub struct ImportedAuthUser;

impl AstCheck for ImportedAuthUser {
    fn code(&self) -> &'static str {
        "E5142"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            if let Stmt::ImportFrom(imp) = stmt {
                let module = imp.module.as_ref().map(|m| m.as_str()).unwrap_or("");
                if module == "django.contrib.auth.models" {
                    let imports_user = imp.names.iter().any(|alias| alias.name.as_str() == "User");
                    if imports_user {
                        diags.push(
                            Diagnostic::new(
                                "E5142",
                                "Importing User from django.contrib.auth.models is discouraged — use get_user_model() instead.",
                                ctx.filename,
                            )
                            .with_range(imp.range()),
                        );
                    }
                }
            }
        }
        diags
    }
}

// ── R5101: HttpResponseWithJsonDumps ──────────────────────────────────────

pub struct HttpResponseWithJsonDumps;

impl AstCheck for HttpResponseWithJsonDumps {
    fn code(&self) -> &'static str {
        "R5101"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = HttpResponseJsonDumpsVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct HttpResponseJsonDumpsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for HttpResponseJsonDumpsVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if is_http_response(call.func.as_ref()) {
                if let Some(first_arg) = call.arguments.args.first() {
                    if is_json_dumps(first_arg) {
                        self.diags.push(
                            Diagnostic::new(
                                "R5101",
                                "Use JsonResponse(data) instead of HttpResponse(json.dumps(data)).",
                                self.filename,
                            )
                            .with_range(call.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── R5102: HttpResponseWithContentTypeJson ────────────────────────────────

pub struct HttpResponseWithContentTypeJson;

impl AstCheck for HttpResponseWithContentTypeJson {
    fn code(&self) -> &'static str {
        "R5102"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = HttpResponseContentTypeJsonVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct HttpResponseContentTypeJsonVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for HttpResponseContentTypeJsonVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if is_http_response(call.func.as_ref()) {
                let has_json_content_type = call.arguments.keywords.iter().any(|kw| {
                    kw.arg
                        .as_ref()
                        .is_some_and(|a| a.as_str() == "content_type")
                        && is_application_json_string(&kw.value)
                });
                if has_json_content_type {
                    self.diags.push(
                        Diagnostic::new(
                            "R5102",
                            "Use JsonResponse() instead of HttpResponse(content_type='application/json').",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── R5103: RedundantContentTypeForJsonResponse ────────────────────────────

pub struct RedundantContentTypeForJsonResponse;

impl AstCheck for RedundantContentTypeForJsonResponse {
    fn code(&self) -> &'static str {
        "R5103"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = RedundantContentTypeJsonVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RedundantContentTypeJsonVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for RedundantContentTypeJsonVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            if is_json_response(call.func.as_ref()) {
                let has_content_type = call.arguments.keywords.iter().any(|kw| {
                    kw.arg
                        .as_ref()
                        .is_some_and(|a| a.as_str() == "content_type")
                });
                if has_content_type {
                    self.diags.push(
                        Diagnostic::new(
                            "R5103",
                            "Redundant content_type parameter for JsonResponse().",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── W5197: MissingBackwardsMigrationCallable ──────────────────────────────

pub struct MissingBackwardsMigrationCallable;

impl AstCheck for MissingBackwardsMigrationCallable {
    fn code(&self) -> &'static str {
        "W5197"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Only run on migration files
        if !ctx.filename.contains("/migrations/") && !ctx.filename.contains("\\migrations\\") {
            return vec![];
        }
        let mut v = RunPythonVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct RunPythonVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for RunPythonVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let is_run_python = match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "RunPython",
                Expr::Attribute(a) => a.attr.as_str() == "RunPython",
                _ => false,
            };
            if is_run_python {
                let positional_only = call.arguments.args.len() == 1;
                let has_reverse = call.arguments.keywords.iter().any(|kw| {
                    kw.arg
                        .as_ref()
                        .is_some_and(|a| a.as_str() == "reverse_code")
                });
                if positional_only && !has_reverse {
                    self.diags.push(
                        Diagnostic::new(
                            "W5197",
                            "RunPython migration operation is missing a reverse_code argument.",
                            self.filename,
                        )
                        .with_range(call.range()),
                    );
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

// ── W5198: NewDbFieldWithDefault ─────────────────────────────────────────

pub struct NewDbFieldWithDefault;

impl AstCheck for NewDbFieldWithDefault {
    fn code(&self) -> &'static str {
        "W5198"
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if !ctx.filename.contains("/migrations/") && !ctx.filename.contains("\\migrations\\") {
            return vec![];
        }
        let mut v = AddFieldDefaultVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct AddFieldDefaultVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

impl<'a> Visitor<'_> for AddFieldDefaultVisitor<'a> {
    fn visit_expr(&mut self, expr: &Expr) {
        if let Expr::Call(call) = expr {
            let is_add_field = match call.func.as_ref() {
                Expr::Name(n) => n.id.as_str() == "AddField",
                Expr::Attribute(a) => a.attr.as_str() == "AddField",
                _ => false,
            };
            if is_add_field {
                // Look for the `field` keyword argument
                for kw in &call.arguments.keywords {
                    if kw.arg.as_ref().is_some_and(|a| a.as_str() == "field") {
                        // Check if the field call has a `default` keyword
                        if let Expr::Call(field_call) = &kw.value {
                            let has_default = field_call.arguments.keywords.iter().any(|fkw| {
                                fkw.arg.as_ref().is_some_and(|a| a.as_str() == "default")
                            });
                            if has_default {
                                self.diags.push(
                                    Diagnostic::new(
                                        "W5198",
                                        "AddField migration sets a default value on the field — causes full-table rewrite on large tables.",
                                        self.filename,
                                    )
                                    .with_range(call.range()),
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

// ── DJ034: TooManyArguments ───────────────────────────────────────────────

pub struct TooManyArguments {
    pub max_args: u32,
}

impl AstCheck for TooManyArguments {
    fn code(&self) -> &'static str {
        "DJ034"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = TooManyArgumentsVisitor {
            diags: vec![],
            filename: ctx.filename,
            max_args: self.max_args,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct TooManyArgumentsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    max_args: u32,
}

impl<'a> Visitor<'_> for TooManyArgumentsVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            let name = f.name.as_str();
            // Skip constructors — they often legitimately need many args
            if name != "__init__" {
                let param_count = f
                    .parameters
                    .args
                    .iter()
                    .filter(|p| {
                        let pname = p.parameter.name.as_str();
                        pname != "self" && pname != "cls"
                    })
                    .count();
                if param_count > self.max_args as usize {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ034",
                            format!("Function '{name}' has {param_count} arguments (max {}). Consider using a config object or **kwargs.", self.max_args),
                            self.filename,
                        )
                        .with_range(f.range()),
                    );
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ035: TooManyReturnStatements ────────────────────────────────────────

pub struct TooManyReturnStatements {
    pub max_returns: u32,
}

impl AstCheck for TooManyReturnStatements {
    fn code(&self) -> &'static str {
        "DJ035"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = TooManyReturnsVisitor {
            diags: vec![],
            filename: ctx.filename,
            max_returns: self.max_returns,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct TooManyReturnsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    max_returns: u32,
}

impl<'a> Visitor<'_> for TooManyReturnsVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            let name = f.name.as_str().to_string();
            let count = count_returns_in_body(&f.body);
            if count > self.max_returns as usize {
                self.diags.push(
                    Diagnostic::new(
                        "DJ035",
                        format!("Function '{name}' has {count} return statements (max {}). Consider simplifying.", self.max_returns),
                        self.filename,
                    )
                    .with_range(f.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

fn count_returns_in_body(body: &[Stmt]) -> usize {
    let mut count = 0;
    for stmt in body {
        count += count_returns_in_stmt(stmt);
    }
    count
}

fn count_returns_in_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::Return(_) => 1,
        Stmt::If(s) => {
            count_returns_in_body(&s.body)
                + s.elif_else_clauses
                    .iter()
                    .map(|c| count_returns_in_body(&c.body))
                    .sum::<usize>()
        }
        Stmt::For(s) => count_returns_in_body(&s.body) + count_returns_in_body(&s.orelse),
        Stmt::While(s) => count_returns_in_body(&s.body) + count_returns_in_body(&s.orelse),
        Stmt::With(s) => count_returns_in_body(&s.body),
        Stmt::Try(s) => {
            count_returns_in_body(&s.body)
                + s.handlers
                    .iter()
                    .map(|h| {
                        let ExceptHandler::ExceptHandler(eh) = h;
                        count_returns_in_body(&eh.body)
                    })
                    .sum::<usize>()
                + count_returns_in_body(&s.orelse)
                + count_returns_in_body(&s.finalbody)
        }
        // Don't descend into nested function/class defs — those have their own checks
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => 0,
        _ => 0,
    }
}

// ── DJ036: TooManyBranches ────────────────────────────────────────────────

pub struct TooManyBranches {
    pub max_branches: u32,
}

impl AstCheck for TooManyBranches {
    fn code(&self) -> &'static str {
        "DJ036"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Test functions are legitimately long; skip them to avoid noise
        if ctx.filename.contains("/tests/") {
            return vec![];
        }
        let mut v = TooManyBranchesVisitor {
            diags: vec![],
            filename: ctx.filename,
            max_branches: self.max_branches,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct TooManyBranchesVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    max_branches: u32,
}

impl<'a> Visitor<'_> for TooManyBranchesVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            let name = f.name.as_str().to_string();
            let count = count_branches_in_body(&f.body);
            if count > self.max_branches as usize {
                self.diags.push(
                    Diagnostic::new(
                        "DJ036",
                        format!("Function '{name}' has {count} branches (max {}). Consider breaking into smaller functions.", self.max_branches),
                        self.filename,
                    )
                    .with_range(f.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

fn count_branches_in_body(body: &[Stmt]) -> usize {
    body.iter().map(count_branches_in_stmt).sum()
}

fn count_branches_in_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::If(s) => {
            // Count the if itself plus each elif/else branch
            let elif_else_count = s.elif_else_clauses.len();
            1 + elif_else_count
                + count_branches_in_body(&s.body)
                + s.elif_else_clauses
                    .iter()
                    .map(|c| count_branches_in_body(&c.body))
                    .sum::<usize>()
        }
        Stmt::For(s) => 1 + count_branches_in_body(&s.body) + count_branches_in_body(&s.orelse),
        Stmt::While(s) => 1 + count_branches_in_body(&s.body) + count_branches_in_body(&s.orelse),
        Stmt::Try(s) => {
            let handler_count = s.handlers.len();
            1 + handler_count
                + count_branches_in_body(&s.body)
                + s.handlers
                    .iter()
                    .map(|h| {
                        let ExceptHandler::ExceptHandler(eh) = h;
                        count_branches_in_body(&eh.body)
                    })
                    .sum::<usize>()
                + count_branches_in_body(&s.orelse)
                + count_branches_in_body(&s.finalbody)
        }
        Stmt::With(s) => count_branches_in_body(&s.body),
        // Don't descend into nested function/class defs
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => 0,
        _ => 0,
    }
}

// ── DJ037: TooManyLocalVariables ──────────────────────────────────────────

pub struct TooManyLocalVariables {
    pub max_locals: u32,
}

impl AstCheck for TooManyLocalVariables {
    fn code(&self) -> &'static str {
        "DJ037"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Test functions are legitimately long; skip them to avoid noise
        if ctx.filename.contains("/tests/") {
            return vec![];
        }
        let mut v = TooManyLocalsVisitor {
            diags: vec![],
            filename: ctx.filename,
            max_locals: self.max_locals,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct TooManyLocalsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    max_locals: u32,
}

impl<'a> Visitor<'_> for TooManyLocalsVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            let name = f.name.as_str().to_string();
            let mut locals: std::collections::HashSet<String> = std::collections::HashSet::new();
            collect_local_vars_from_body(&f.body, &mut locals);
            let count = locals.len();
            if count > self.max_locals as usize {
                self.diags.push(
                    Diagnostic::new(
                        "DJ037",
                        format!("Function '{name}' has {count} local variables (max {}). Consider extracting helper functions.", self.max_locals),
                        self.filename,
                    )
                    .with_range(f.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

fn collect_local_vars_from_body(body: &[Stmt], locals: &mut std::collections::HashSet<String>) {
    for stmt in body {
        collect_local_vars_from_stmt(stmt, locals);
    }
}

fn collect_local_vars_from_stmt(stmt: &Stmt, locals: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Assign(s) => {
            for target in &s.targets {
                collect_name_targets(target, locals);
            }
        }
        Stmt::AugAssign(s) => {
            collect_name_targets(&s.target, locals);
        }
        Stmt::AnnAssign(s) => {
            collect_name_targets(&s.target, locals);
        }
        Stmt::If(s) => {
            collect_local_vars_from_body(&s.body, locals);
            for clause in &s.elif_else_clauses {
                collect_local_vars_from_body(&clause.body, locals);
            }
        }
        Stmt::For(s) => {
            collect_name_targets(&s.target, locals);
            collect_local_vars_from_body(&s.body, locals);
            collect_local_vars_from_body(&s.orelse, locals);
        }
        Stmt::While(s) => {
            collect_local_vars_from_body(&s.body, locals);
            collect_local_vars_from_body(&s.orelse, locals);
        }
        Stmt::With(s) => {
            for item in &s.items {
                if let Some(var) = &item.optional_vars {
                    collect_name_targets(var, locals);
                }
            }
            collect_local_vars_from_body(&s.body, locals);
        }
        Stmt::Try(s) => {
            collect_local_vars_from_body(&s.body, locals);
            for handler in &s.handlers {
                let ExceptHandler::ExceptHandler(eh) = handler;
                if let Some(name) = &eh.name {
                    locals.insert(name.as_str().to_string());
                }
                collect_local_vars_from_body(&eh.body, locals);
            }
            collect_local_vars_from_body(&s.orelse, locals);
            collect_local_vars_from_body(&s.finalbody, locals);
        }
        // Don't descend into nested function/class defs
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
        _ => {}
    }
}

fn collect_name_targets(expr: &Expr, locals: &mut std::collections::HashSet<String>) {
    match expr {
        Expr::Name(n) => {
            let name = n.id.as_str();
            // Exclude self.x style access (though bare Name won't be self.x anyway)
            if name != "self" && name != "cls" {
                locals.insert(name.to_string());
            }
        }
        Expr::Tuple(t) => {
            for elt in &t.elts {
                collect_name_targets(elt, locals);
            }
        }
        Expr::List(l) => {
            for elt in &l.elts {
                collect_name_targets(elt, locals);
            }
        }
        Expr::Starred(s) => {
            collect_name_targets(&s.value, locals);
        }
        _ => {}
    }
}

// ── DJ038: TooManyStatements ──────────────────────────────────────────────

pub struct TooManyStatements {
    pub max_statements: u32,
}

impl AstCheck for TooManyStatements {
    fn code(&self) -> &'static str {
        "DJ038"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Test functions are legitimately long; skip them to avoid noise
        if ctx.filename.contains("/tests/") {
            return vec![];
        }
        let mut v = TooManyStatementsVisitor {
            diags: vec![],
            filename: ctx.filename,
            max_statements: self.max_statements,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct TooManyStatementsVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    max_statements: u32,
}

impl<'a> Visitor<'_> for TooManyStatementsVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            let name = f.name.as_str().to_string();
            let count = count_statements_in_body(&f.body);
            if count > self.max_statements as usize {
                self.diags.push(
                    Diagnostic::new(
                        "DJ038",
                        format!("Function '{name}' has {count} statements (max {}). Consider breaking into smaller functions.", self.max_statements),
                        self.filename,
                    )
                    .with_range(f.range()),
                );
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

fn count_statements_in_body(body: &[Stmt]) -> usize {
    body.iter().map(count_statements_in_stmt).sum()
}

fn count_statements_in_stmt(stmt: &Stmt) -> usize {
    match stmt {
        Stmt::If(s) => {
            1 + count_statements_in_body(&s.body)
                + s.elif_else_clauses
                    .iter()
                    .map(|c| count_statements_in_body(&c.body))
                    .sum::<usize>()
        }
        Stmt::For(s) => 1 + count_statements_in_body(&s.body) + count_statements_in_body(&s.orelse),
        Stmt::While(s) => {
            1 + count_statements_in_body(&s.body) + count_statements_in_body(&s.orelse)
        }
        Stmt::With(s) => 1 + count_statements_in_body(&s.body),
        Stmt::Try(s) => {
            1 + count_statements_in_body(&s.body)
                + s.handlers
                    .iter()
                    .map(|h| {
                        let ExceptHandler::ExceptHandler(eh) = h;
                        count_statements_in_body(&eh.body)
                    })
                    .sum::<usize>()
                + count_statements_in_body(&s.orelse)
                + count_statements_in_body(&s.finalbody)
        }
        // Nested functions count as 1 statement but don't recurse into them
        Stmt::FunctionDef(_) | Stmt::ClassDef(_) => 1,
        _ => 1,
    }
}

// ── DJ039: ModelTooManyFields ─────────────────────────────────────────────

pub struct ModelTooManyFields {
    pub max_fields: u32,
}

impl AstCheck for ModelTooManyFields {
    fn code(&self) -> &'static str {
        "DJ039"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            if let Stmt::ClassDef(cls) = stmt {
                if is_django_model(cls) {
                    let field_count = cls
                        .body
                        .iter()
                        .filter(|s| is_model_field_assignment(s))
                        .count();
                    if field_count > self.max_fields as usize {
                        let name = cls.name.as_str();
                        diags.push(
                            Diagnostic::new(
                                "DJ039",
                                format!("Model '{name}' has {field_count} fields (max {}). Consider splitting into related models.", self.max_fields),
                                ctx.filename,
                            )
                            .with_range(cls.range()),
                        );
                    }
                }
            }
        }
        diags
    }
}

const DJANGO_FIELD_NAMES: &[&str] = &[
    "AutoField",
    "BigAutoField",
    "SmallAutoField",
    "BooleanField",
    "NullBooleanField",
    "CharField",
    "TextField",
    "EmailField",
    "URLField",
    "SlugField",
    "FilePathField",
    "FileField",
    "ImageField",
    "IntegerField",
    "BigIntegerField",
    "SmallIntegerField",
    "PositiveIntegerField",
    "PositiveSmallIntegerField",
    "PositiveBigIntegerField",
    "FloatField",
    "DecimalField",
    "DateField",
    "DateTimeField",
    "TimeField",
    "DurationField",
    "BinaryField",
    "UUIDField",
    "GenericIPAddressField",
    "IPAddressField",
    "JSONField",
    "ForeignKey",
    "OneToOneField",
    "ManyToManyField",
];

fn is_model_field_assignment(stmt: &Stmt) -> bool {
    if let Stmt::Assign(assign) = stmt {
        if let Expr::Call(call) = assign.value.as_ref() {
            let fname = match call.func.as_ref() {
                Expr::Name(n) => Some(n.id.as_str()),
                Expr::Attribute(a) => Some(a.attr.as_str()),
                _ => None,
            };
            if let Some(name) = fname {
                return DJANGO_FIELD_NAMES.contains(&name);
            }
        }
    }
    false
}

// ── DJ040: TooManyMethods ─────────────────────────────────────────────────

pub struct TooManyMethods {
    pub max_methods: u32,
}

impl AstCheck for TooManyMethods {
    fn code(&self) -> &'static str {
        "DJ040"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            if let Stmt::ClassDef(cls) = stmt {
                if is_drf_view(cls) || is_serializer_class(cls) {
                    let method_count = cls
                        .body
                        .iter()
                        .filter(|s| matches!(s, Stmt::FunctionDef(_)))
                        .count();
                    if method_count > self.max_methods as usize {
                        let name = cls.name.as_str();
                        diags.push(
                            Diagnostic::new(
                                "DJ040",
                                format!("Class '{name}' has {method_count} methods (max {}). Consider using mixins or splitting.", self.max_methods),
                                ctx.filename,
                            )
                            .with_range(cls.range()),
                        );
                    }
                }
            }
        }
        diags
    }
}

// ── DJ041: DeeplyNestedCode ───────────────────────────────────────────────

pub struct DeeplyNestedCode {
    pub max_depth: u32,
}

impl AstCheck for DeeplyNestedCode {
    fn code(&self) -> &'static str {
        "DJ041"
    }
    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::All
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        // Test functions are legitimately deeply nested (subTest blocks, multi-scenario tests)
        if ctx.filename.contains("/tests/") {
            return vec![];
        }
        let mut v = DeeplyNestedVisitor {
            diags: vec![],
            filename: ctx.filename,
            depth: 0,
            max_depth: self.max_depth,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct DeeplyNestedVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
    depth: usize,
    max_depth: u32,
}

impl<'a> DeeplyNestedVisitor<'a> {
    fn check_body(&mut self, body: &[Stmt], range: ruff_text_size::TextRange) {
        self.depth += 1;
        if self.depth > self.max_depth as usize {
            self.diags.push(
                Diagnostic::new(
                    "DJ041",
                    format!("Code is nested {} levels deep (max {}). Consider early returns or extracting functions.", self.depth, self.max_depth),
                    self.filename,
                )
                .with_range(range),
            );
        }
        for stmt in body {
            self.visit_nesting_stmt(stmt);
        }
        self.depth -= 1;
    }

    fn visit_nesting_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::If(s) => {
                self.check_body(&s.body, s.range());
                for clause in &s.elif_else_clauses {
                    self.check_body(&clause.body, clause.range());
                }
            }
            Stmt::For(s) => {
                self.check_body(&s.body, s.range());
                if !s.orelse.is_empty() {
                    self.check_body(&s.orelse, s.range());
                }
            }
            Stmt::While(s) => {
                self.check_body(&s.body, s.range());
                if !s.orelse.is_empty() {
                    self.check_body(&s.orelse, s.range());
                }
            }
            Stmt::With(s) => {
                self.check_body(&s.body, s.range());
            }
            Stmt::Try(s) => {
                self.check_body(&s.body, s.range());
                for handler in &s.handlers {
                    let ExceptHandler::ExceptHandler(eh) = handler;
                    self.check_body(&eh.body, eh.range());
                }
                if !s.orelse.is_empty() {
                    self.check_body(&s.orelse, s.range());
                }
                if !s.finalbody.is_empty() {
                    self.check_body(&s.finalbody, s.range());
                }
            }
            // Nested functions reset the depth counter
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => {}
        }
    }
}

impl<'a> Visitor<'_> for DeeplyNestedVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            // Each function body starts at depth 0
            let saved_depth = self.depth;
            self.depth = 0;
            for s in &f.body {
                self.visit_nesting_stmt(s);
            }
            self.depth = saved_depth;
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ044: SuperInitNotCalled ─────────────────────────────────────────────

pub struct SuperInitNotCalled;

impl AstCheck for SuperInitNotCalled {
    fn code(&self) -> &'static str {
        "DJ044"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        if ctx.filename.contains("/tests/") || ctx.filename.contains("\\tests\\") {
            return vec![];
        }
        let mut diags = Vec::new();
        for stmt in &ctx.module.body {
            if let Stmt::ClassDef(cls) = stmt {
                check_super_init_in_class(cls, ctx.filename, &mut diags);
            }
        }
        diags
    }
}

fn class_has_meaningful_bases(cls: &StmtClassDef) -> bool {
    let Some(args) = &cls.arguments else {
        return false;
    };
    args.args.iter().any(|base| match base {
        Expr::Name(n) => {
            let name = n.id.as_str();
            name != "object"
        }
        Expr::Attribute(_) => true,
        _ => false,
    })
}

fn body_calls_super_init(body: &[Stmt]) -> bool {
    for stmt in body {
        if stmt_calls_super_init(stmt) {
            return true;
        }
    }
    false
}

fn stmt_calls_super_init(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Expr(expr_stmt) => expr_calls_super_init(&expr_stmt.value),
        Stmt::If(s) => {
            body_calls_super_init(&s.body)
                || s.elif_else_clauses
                    .iter()
                    .any(|c| body_calls_super_init(&c.body))
        }
        Stmt::Try(s) => {
            body_calls_super_init(&s.body)
                || s.handlers.iter().any(|h| {
                    let ExceptHandler::ExceptHandler(eh) = h;
                    body_calls_super_init(&eh.body)
                })
        }
        _ => false,
    }
}

fn expr_calls_super_init(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        // super().__init__(...)
        if let Expr::Attribute(attr) = call.func.as_ref() {
            if attr.attr.as_str() == "__init__" {
                // Check if the receiver is super() or SuperClass
                match attr.value.as_ref() {
                    Expr::Call(inner) => {
                        if let Expr::Name(n) = inner.func.as_ref() {
                            if n.id.as_str() == "super" {
                                return true;
                            }
                        }
                    }
                    // ParentClass.__init__(self, ...) style
                    Expr::Name(_) | Expr::Attribute(_) => return true,
                    _ => {}
                }
            }
        }
    }
    false
}

fn check_super_init_in_class(cls: &StmtClassDef, filename: &str, diags: &mut Vec<Diagnostic>) {
    if !class_has_meaningful_bases(cls) {
        return;
    }
    for stmt in &cls.body {
        if let Stmt::FunctionDef(func) = stmt {
            if func.name.as_str() == "__init__" && !body_calls_super_init(&func.body) {
                let class_name = cls.name.as_str();
                diags.push(
                        Diagnostic::new(
                            "DJ044",
                            format!("'__init__' in class '{class_name}' does not call super().__init__(). This can cause MRO issues with Django classes."),
                            filename,
                        )
                        .with_range(func.range()),
                    );
            }
        }
    }
}

// ── DJ045: BadExceptOrder ─────────────────────────────────────────────────

pub struct BadExceptOrder;

impl AstCheck for BadExceptOrder {
    fn code(&self) -> &'static str {
        "DJ045"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = BadExceptOrderVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct BadExceptOrderVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

/// Returns true if `parent_name` is a known parent exception of `child_name`.
fn is_broader_exception(parent_name: &str, child_name: &str) -> bool {
    match parent_name {
        "Exception" => matches!(
            child_name,
            "ValueError"
                | "TypeError"
                | "KeyError"
                | "IndexError"
                | "AttributeError"
                | "RuntimeError"
                | "OSError"
                | "IOError"
                | "ImportError"
                | "StopIteration"
                | "ArithmeticError"
                | "LookupError"
                | "ZeroDivisionError"
                | "OverflowError"
                | "FloatingPointError"
                | "FileNotFoundError"
                | "PermissionError"
                | "FileExistsError"
                | "IsADirectoryError"
                | "NotADirectoryError"
                | "ConnectionError"
                | "TimeoutError"
                | "ConnectionRefusedError"
                | "ConnectionResetError"
                | "BrokenPipeError"
                | "ConnectionAbortedError"
                | "UnicodeError"
        ),
        "LookupError" => matches!(child_name, "KeyError" | "IndexError"),
        "ArithmeticError" => matches!(
            child_name,
            "ZeroDivisionError" | "OverflowError" | "FloatingPointError"
        ),
        "OSError" | "IOError" => matches!(
            child_name,
            "FileNotFoundError"
                | "PermissionError"
                | "FileExistsError"
                | "IsADirectoryError"
                | "NotADirectoryError"
                | "ConnectionError"
                | "TimeoutError"
                | "ConnectionRefusedError"
                | "ConnectionResetError"
                | "BrokenPipeError"
                | "ConnectionAbortedError"
        ),
        "ConnectionError" => matches!(
            child_name,
            "ConnectionRefusedError"
                | "ConnectionResetError"
                | "BrokenPipeError"
                | "ConnectionAbortedError"
        ),
        "ValueError" => child_name == "UnicodeError",
        _ => false,
    }
}

fn except_handler_type_name(handler: &ExceptHandler) -> Option<&str> {
    let ExceptHandler::ExceptHandler(eh) = handler;
    match eh.type_.as_deref()? {
        Expr::Name(n) => Some(n.id.as_str()),
        Expr::Attribute(a) => Some(a.attr.as_str()),
        _ => None,
    }
}

impl<'a> Visitor<'_> for BadExceptOrderVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Try(try_stmt) = stmt {
            let handlers = &try_stmt.handlers;
            for i in 0..handlers.len() {
                let Some(earlier_name) = except_handler_type_name(&handlers[i]) else {
                    visitor::walk_stmt(self, stmt);
                    return;
                };
                for later_handler in handlers.iter().skip(i + 1) {
                    let Some(later_name) = except_handler_type_name(later_handler) else {
                        continue;
                    };
                    if is_broader_exception(earlier_name, later_name) {
                        let ExceptHandler::ExceptHandler(eh) = later_handler;
                        self.diags.push(
                            Diagnostic::new(
                                "DJ045",
                                format!(
                                    "Exception handler '{earlier_name}' is broader than later handler '{later_name}' — '{later_name}' is unreachable."
                                ),
                                self.filename,
                            )
                            .with_range(eh.range()),
                        );
                    }
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ046: UsingConstantTest ──────────────────────────────────────────────

pub struct UsingConstantTest;

impl AstCheck for UsingConstantTest {
    fn code(&self) -> &'static str {
        "DJ046"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Improve
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = UsingConstantTestVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct UsingConstantTestVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

/// Returns a human-readable description of the constant and whether it is
/// a constant we should flag, or None if it should be skipped.
fn constant_test_description(expr: &Expr) -> Option<String> {
    match expr {
        Expr::BooleanLiteral(b) => Some(if b.value {
            "True".to_string()
        } else {
            "False".to_string()
        }),
        Expr::NumberLiteral(n) => {
            // Only flag non-zero numeric constants — `while 1:` is idiomatic in some codebases
            // but we keep this check consistent with pylint W0125
            Some(format!("{:?}", n.value))
        }
        Expr::StringLiteral(s) => {
            let val = s.value.to_str();
            if val.is_empty() {
                None
            } else {
                Some(format!("'{val}'"))
            }
        }
        Expr::NoneLiteral(_) => Some("None".to_string()),
        _ => None,
    }
}

fn is_type_checking_name(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(a) => a.attr.as_str() == "TYPE_CHECKING",
        _ => false,
    }
}

fn is_dunder_name_check(expr: &Expr) -> bool {
    // `if __name__ == "__main__":` — skip
    if let Expr::Compare(cmp) = expr {
        if let Expr::Name(n) = cmp.left.as_ref() {
            if n.id.as_str() == "__name__" {
                return true;
            }
        }
    }
    false
}

impl<'a> Visitor<'_> for UsingConstantTestVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::If(if_stmt) => {
                let test = &if_stmt.test;
                // Skip `if TYPE_CHECKING:` and `if __name__ == "__main__":`
                if !is_type_checking_name(test) && !is_dunder_name_check(test) {
                    if let Some(desc) = constant_test_description(test) {
                        self.diags.push(
                            Diagnostic::new(
                                "DJ046",
                                format!(
                                    "Using constant test '{desc}' — this branch may be dead code."
                                ),
                                self.filename,
                            )
                            .with_range(if_stmt.range()),
                        );
                    }
                }
            }
            Stmt::While(while_stmt) => {
                let test = &while_stmt.test;
                // `while True:` is a recognised idiom — skip it
                if let Expr::BooleanLiteral(b) = &**test {
                    if b.value {
                        visitor::walk_stmt(self, stmt);
                        return;
                    }
                }
                if let Some(desc) = constant_test_description(test) {
                    self.diags.push(
                        Diagnostic::new(
                            "DJ046",
                            format!("Using constant test '{desc}' in while loop — this loop is dead code."),
                            self.filename,
                        )
                        .with_range(while_stmt.range()),
                    );
                }
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── DJ048: SelfAssigningVariable ──────────────────────────────────────────

pub struct SelfAssigningVariable;

impl AstCheck for SelfAssigningVariable {
    fn code(&self) -> &'static str {
        "DJ048"
    }

    fn level(&self) -> thorn_api::Level {
        thorn_api::Level::Fix
    }

    fn check(&self, ctx: &CheckContext) -> Vec<Diagnostic> {
        let mut v = SelfAssigningVisitor {
            diags: vec![],
            filename: ctx.filename,
        };
        v.visit_body(&ctx.module.body);
        v.diags
    }
}

struct SelfAssigningVisitor<'a> {
    diags: Vec<Diagnostic>,
    filename: &'a str,
}

/// Returns the "canonical string key" of an expression if it is a simple
/// name or attribute access chain we can compare for self-assignment.
/// Examples: `x` → `"x"`, `self.name` → `"self.name"`.
fn expr_as_assign_key(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(n) => Some(n.id.as_str().to_string()),
        Expr::Attribute(a) => {
            let base = expr_as_assign_key(&a.value)?;
            Some(format!("{}.{}", base, a.attr.as_str()))
        }
        _ => None,
    }
}

impl<'a> Visitor<'_> for SelfAssigningVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::Assign(assign) = stmt {
            // Skip tuple/multi-target assignments — too complex to check simply
            if assign.targets.len() == 1 {
                if let Some(target_key) = expr_as_assign_key(&assign.targets[0]) {
                    if let Some(value_key) = expr_as_assign_key(&assign.value) {
                        if target_key == value_key {
                            self.diags.push(
                                Diagnostic::new(
                                    "DJ048",
                                    format!("'{target_key} = {value_key}' is a self-assignment (likely a typo)."),
                                    self.filename,
                                )
                                .with_range(assign.range()),
                            );
                        }
                    }
                }
            }
        }
        visitor::walk_stmt(self, stmt);
    }
}

// ── Helper functions ──────────────────────────────────────────────────────

/// Returns true if the file is a seeder or fixture file.
/// Seeder/fixture files intentionally create records in loops and use random ordering.
fn is_seeder_or_fixture(filename: &str) -> bool {
    // Extract just the filename (last path component) for matching
    let basename = filename.rsplit('/').next().unwrap_or(filename);
    let stem = basename.strip_suffix(".py").unwrap_or(basename);
    stem.contains("seed") || stem.contains("fixture")
}

fn is_self_access(expr: &Expr) -> bool {
    matches!(expr, Expr::Name(n) if n.id.as_str() == "self")
}

/// Returns true if the class has an inner `Meta` class containing `abstract = True`.
/// Abstract models should not be required to define `__str__`.
fn has_abstract_meta(class: &StmtClassDef) -> bool {
    class.body.iter().any(|stmt| {
        if let Stmt::ClassDef(meta) = stmt {
            if meta.name.as_str() == "Meta" {
                return meta.body.iter().any(|s| {
                    if let Stmt::Assign(assign) = s {
                        assign.targets.iter().any(|t| {
                            if let Expr::Name(n) = t {
                                n.id.as_str() == "abstract"
                                    && matches!(&*assign.value, Expr::BooleanLiteral(b) if b.value)
                            } else {
                                false
                            }
                        })
                    } else {
                        false
                    }
                });
            }
        }
        false
    })
}

fn is_django_model(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| match base {
            Expr::Attribute(a) => a.attr.as_str() == "Model",
            Expr::Name(n) => n.id.as_str() == "Model",
            _ => false,
        })
    })
}

fn is_model_form(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| match base {
            Expr::Name(n) => n.id.as_str() == "ModelForm",
            Expr::Attribute(a) => a.attr.as_str() == "ModelForm",
            _ => false,
        })
    })
}

fn has_null_true(arguments: &Arguments) -> bool {
    arguments.keywords.iter().any(|kw| {
        kw.arg.as_ref().is_some_and(|a| a.as_str() == "null")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    })
}

fn has_unique_true(arguments: &Arguments) -> bool {
    arguments.keywords.iter().any(|kw| {
        kw.arg.as_ref().is_some_and(|a| a.as_str() == "unique")
            && matches!(&kw.value, Expr::BooleanLiteral(b) if b.value)
    })
}

fn is_locals_call(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        if let Expr::Name(n) = call.func.as_ref() {
            return n.id.as_str() == "locals";
        }
    }
    false
}

fn is_http_response(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "HttpResponse",
        Expr::Attribute(a) => a.attr.as_str() == "HttpResponse",
        _ => false,
    }
}

fn is_json_response(expr: &Expr) -> bool {
    match expr {
        Expr::Name(n) => n.id.as_str() == "JsonResponse",
        Expr::Attribute(a) => a.attr.as_str() == "JsonResponse",
        _ => false,
    }
}

fn is_json_dumps(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        return match call.func.as_ref() {
            Expr::Name(n) => n.id.as_str() == "dumps",
            Expr::Attribute(a) => {
                a.attr.as_str() == "dumps"
                    && matches!(a.value.as_ref(), Expr::Name(n) if n.id.as_str() == "json")
            }
            _ => false,
        };
    }
    false
}

fn is_application_json_string(expr: &Expr) -> bool {
    if let Expr::StringLiteral(s) = expr {
        let val = s.value.to_str();
        return val == "application/json" || val.starts_with("application/json");
    }
    false
}

fn has_objects_in_chain(expr: &Expr) -> bool {
    match expr {
        Expr::Attribute(attr) => {
            if attr.attr.as_str() == "objects" || attr.attr.as_str() == "all_objects" {
                return true;
            }
            has_objects_in_chain(attr.value.as_ref())
        }
        Expr::Call(call) => {
            if let Expr::Attribute(attr) = call.func.as_ref() {
                if attr.attr.as_str() == "objects" || attr.attr.as_str() == "all_objects" {
                    return true;
                }
                has_objects_in_chain(attr.value.as_ref())
            } else {
                false
            }
        }
        _ => false,
    }
}

fn is_queryset_call(expr: &Expr) -> bool {
    if let Expr::Call(call) = expr {
        if let Expr::Attribute(attr) = call.func.as_ref() {
            let method = attr.attr.as_str();
            // .exists() and .count() return scalars (bool/int), not querysets —
            // so `if qs.exists():` is correct and must NOT be flagged.
            if matches!(method, "exists" | "count") {
                return false;
            }
            // Only flag when the call chain is rooted at .objects or .all_objects,
            // which rules out dict.get(), list.first(), etc.
            if has_objects_in_chain(attr.value.as_ref()) {
                return true;
            }
        }
    }
    false
}

fn is_serializer_class(class: &StmtClassDef) -> bool {
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| {
            let name = match base {
                Expr::Name(n) => Some(n.id.as_str()),
                Expr::Attribute(a) => Some(a.attr.as_str()),
                _ => None,
            };
            matches!(
                name,
                Some("ModelSerializer") | Some("HyperlinkedModelSerializer")
            )
        })
    })
}

fn is_drf_view(class: &StmtClassDef) -> bool {
    const DRF_VIEW_BASES: &[&str] = &[
        "APIView",
        "ViewSet",
        "ModelViewSet",
        "ReadOnlyModelViewSet",
        "GenericViewSet",
        "GenericAPIView",
        "ListAPIView",
        "CreateAPIView",
        "RetrieveAPIView",
        "DestroyAPIView",
        "UpdateAPIView",
        "ListCreateAPIView",
        "RetrieveUpdateAPIView",
        "RetrieveDestroyAPIView",
        "RetrieveUpdateDestroyAPIView",
    ];
    class.arguments.as_ref().is_some_and(|args| {
        args.args.iter().any(|base| {
            let name = match base {
                Expr::Name(n) => Some(n.id.as_str()),
                Expr::Attribute(a) => Some(a.attr.as_str()),
                _ => None,
            };
            name.is_some_and(|n| DRF_VIEW_BASES.contains(&n))
        })
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use thorn_api::{AppGraph, AstCheck, CheckContext};

    /// Parse `source` as a Python module, run `check` against it, and return
    /// the list of diagnostic codes produced.
    fn run_check(check: &dyn AstCheck, source: &str) -> Vec<String> {
        let parsed =
            ruff_python_parser::parse(source, ruff_python_parser::Mode::Module.into()).unwrap();
        let module = parsed.into_syntax().module().unwrap().clone();
        let graph = AppGraph::default();
        let ctx = CheckContext {
            module: &module,
            source,
            filename: "test.py",
            graph: &graph,
        };
        check.check(&ctx).into_iter().map(|d| d.code).collect()
    }

    /// Same as `run_check` but allows the caller to supply a custom filename so
    /// that filename-based skip rules (migrations, seed, webhook, …) can be
    /// exercised.
    fn run_check_with_filename(check: &dyn AstCheck, source: &str, filename: &str) -> Vec<String> {
        let parsed =
            ruff_python_parser::parse(source, ruff_python_parser::Mode::Module.into()).unwrap();
        let module = parsed.into_syntax().module().unwrap().clone();
        let graph = AppGraph::default();
        let ctx = CheckContext {
            module: &module,
            source,
            filename,
            graph: &graph,
        };
        check.check(&ctx).into_iter().map(|d| d.code).collect()
    }

    // ── DJ001: NullableStringField ────────────────────────────────────────

    #[test]
    fn dj001_triggers_on_charfield_null_true() {
        let src = "name = models.CharField(max_length=100, null=True)";
        assert!(run_check(&NullableStringField, src).contains(&"DJ001".to_string()));
    }

    #[test]
    fn dj001_triggers_on_textfield_null_true() {
        let src = "bio = models.TextField(null=True)";
        assert!(run_check(&NullableStringField, src).contains(&"DJ001".to_string()));
    }

    #[test]
    fn dj001_triggers_on_emailfield_null_true() {
        let src = "email = models.EmailField(null=True)";
        assert!(run_check(&NullableStringField, src).contains(&"DJ001".to_string()));
    }

    /// null=True is allowed on string fields when unique=True is also present,
    /// because the database needs NULL to distinguish "no value" from the empty
    /// string in a unique index.
    #[test]
    fn dj001_no_trigger_when_unique_also_true() {
        let src = "slug = models.SlugField(max_length=50, null=True, unique=True)";
        assert!(run_check(&NullableStringField, src).is_empty());
    }

    #[test]
    fn dj001_no_trigger_without_null_true() {
        let src = "name = models.CharField(max_length=100, blank=True)";
        assert!(run_check(&NullableStringField, src).is_empty());
    }

    // ── DJ002: ModelFormUsesExclude ───────────────────────────────────────

    #[test]
    fn dj002_triggers_on_modelform_with_exclude() {
        let src = r#"
class ContactForm(ModelForm):
    class Meta:
        model = Contact
        exclude = ["secret"]
"#;
        assert!(run_check(&ModelFormUsesExclude, src).contains(&"DJ002".to_string()));
    }

    #[test]
    fn dj002_no_trigger_when_using_fields() {
        let src = r#"
class ContactForm(ModelForm):
    class Meta:
        model = Contact
        fields = ["name", "email"]
"#;
        assert!(run_check(&ModelFormUsesExclude, src).is_empty());
    }

    /// A non-ModelForm class with an exclude attribute must not be flagged.
    #[test]
    fn dj002_no_trigger_on_plain_class() {
        let src = r#"
class SomeConfig:
    exclude = ["secret"]
"#;
        assert!(run_check(&ModelFormUsesExclude, src).is_empty());
    }

    // ── DJ003: RawSqlUsage ────────────────────────────────────────────────

    #[test]
    fn dj003_triggers_on_raw() {
        let src = r#"qs = MyModel.objects.raw("SELECT * FROM myapp_mymodel")"#;
        assert!(run_check(&RawSqlUsage, src).contains(&"DJ003".to_string()));
    }

    #[test]
    fn dj003_triggers_on_extra() {
        let src = r#"qs = MyModel.objects.extra(select={"n": "SELECT COUNT(*) FROM t"})"#;
        assert!(run_check(&RawSqlUsage, src).contains(&"DJ003".to_string()));
    }

    #[test]
    fn dj003_no_trigger_on_filter() {
        let src = r#"qs = MyModel.objects.filter(active=True)"#;
        assert!(run_check(&RawSqlUsage, src).is_empty());
    }

    // ── DJ004: LocalsInRender ─────────────────────────────────────────────

    #[test]
    fn dj004_triggers_on_render_with_locals() {
        let src = r#"
def my_view(request):
    name = "Alice"
    return render(request, "template.html", locals())
"#;
        assert!(run_check(&LocalsInRender, src).contains(&"DJ004".to_string()));
    }

    #[test]
    fn dj004_no_trigger_on_explicit_context() {
        let src = r#"
def my_view(request):
    name = "Alice"
    return render(request, "template.html", {"name": name})
"#;
        assert!(run_check(&LocalsInRender, src).is_empty());
    }

    // ── DJ005: ModelWithoutStrMethod ──────────────────────────────────────

    #[test]
    fn dj005_triggers_when_str_missing() {
        let src = r#"
class Article(Model):
    title = CharField(max_length=200)
"#;
        assert!(run_check(&ModelWithoutStrMethod, src).contains(&"DJ005".to_string()));
    }

    #[test]
    fn dj005_no_trigger_when_str_present() {
        let src = r#"
class Article(Model):
    title = CharField(max_length=200)

    def __str__(self):
        return self.title
"#;
        assert!(run_check(&ModelWithoutStrMethod, src).is_empty());
    }

    /// Abstract models are skipped — they are designed to be subclassed and
    /// the concrete subclass should define __str__.
    #[test]
    fn dj005_no_trigger_on_abstract_model() {
        let src = r#"
class BaseModel(Model):
    class Meta:
        abstract = True
"#;
        assert!(run_check(&ModelWithoutStrMethod, src).is_empty());
    }

    // ── DJ006: ForeignKeyMissingOnDelete ──────────────────────────────────

    #[test]
    fn dj006_triggers_on_foreignkey_without_on_delete() {
        let src = r#"author = models.ForeignKey(User)"#;
        assert!(run_check(&ForeignKeyMissingOnDelete, src).contains(&"DJ006".to_string()));
    }

    #[test]
    fn dj006_triggers_on_onetoonefield_without_on_delete() {
        let src = r#"profile = models.OneToOneField(User)"#;
        assert!(run_check(&ForeignKeyMissingOnDelete, src).contains(&"DJ006".to_string()));
    }

    #[test]
    fn dj006_no_trigger_when_on_delete_present() {
        let src = r#"author = models.ForeignKey(User, on_delete=models.CASCADE)"#;
        assert!(run_check(&ForeignKeyMissingOnDelete, src).is_empty());
    }

    // ── DJ007: ModelFormFieldsAll ─────────────────────────────────────────

    #[test]
    fn dj007_triggers_on_fields_all_in_modelform() {
        let src = r#"
class ContactForm(ModelForm):
    class Meta:
        model = Contact
        fields = "__all__"
"#;
        assert!(run_check(&ModelFormFieldsAll, src).contains(&"DJ007".to_string()));
    }

    #[test]
    fn dj007_triggers_on_fields_all_in_serializer() {
        let src = r#"
class ContactSerializer(ModelSerializer):
    class Meta:
        model = Contact
        fields = "__all__"
"#;
        assert!(run_check(&ModelFormFieldsAll, src).contains(&"DJ007".to_string()));
    }

    #[test]
    fn dj007_no_trigger_on_explicit_field_list() {
        let src = r#"
class ContactForm(ModelForm):
    class Meta:
        model = Contact
        fields = ["name", "email"]
"#;
        assert!(run_check(&ModelFormFieldsAll, src).is_empty());
    }

    // ── DJ008: RandomOrderBy ──────────────────────────────────────────────

    #[test]
    fn dj008_triggers_on_order_by_question_mark() {
        let src = r#"qs = MyModel.objects.order_by("?")"#;
        assert!(run_check(&RandomOrderBy, src).contains(&"DJ008".to_string()));
    }

    #[test]
    fn dj008_no_trigger_on_normal_order_by() {
        let src = r#"qs = MyModel.objects.order_by("-created_at")"#;
        assert!(run_check(&RandomOrderBy, src).is_empty());
    }

    /// Seed files intentionally use order_by("?") for data variety — the check
    /// must be suppressed for them.
    #[test]
    fn dj008_skipped_in_seed_file() {
        let src = r#"qs = MyModel.objects.order_by("?")"#;
        assert!(run_check_with_filename(&RandomOrderBy, src, "db/seed_users.py").is_empty());
    }

    // ── DJ009: QuerysetBoolEval ───────────────────────────────────────────

    #[test]
    fn dj009_triggers_on_if_queryset_bool_check() {
        let src = r#"
if MyModel.objects.filter(active=True):
    pass
"#;
        assert!(run_check(&QuerysetBoolEval, src).contains(&"DJ009".to_string()));
    }

    #[test]
    fn dj009_triggers_on_negated_queryset_bool_check() {
        let src = r#"
if not MyModel.objects.filter(active=True):
    pass
"#;
        assert!(run_check(&QuerysetBoolEval, src).contains(&"DJ009".to_string()));
    }

    #[test]
    fn dj009_no_trigger_on_if_exists() {
        // .exists() returns a bool, not a queryset — this is the correct pattern.
        let src = r#"
if MyModel.objects.filter(active=True).exists():
    pass
"#;
        assert!(run_check(&QuerysetBoolEval, src).is_empty());
    }

    // ── DJ010: QuerysetLen ────────────────────────────────────────────────

    #[test]
    fn dj010_triggers_on_len_of_queryset() {
        let src = r#"n = len(MyModel.objects.all())"#;
        assert!(run_check(&QuerysetLen, src).contains(&"DJ010".to_string()));
    }

    #[test]
    fn dj010_triggers_on_len_of_filtered_queryset() {
        let src = r#"n = len(MyModel.objects.filter(active=True))"#;
        assert!(run_check(&QuerysetLen, src).contains(&"DJ010".to_string()));
    }

    #[test]
    fn dj010_no_trigger_on_len_of_list() {
        let src = r#"n = len([1, 2, 3])"#;
        assert!(run_check(&QuerysetLen, src).is_empty());
    }

    #[test]
    fn dj010_no_trigger_on_count() {
        let src = r#"n = MyModel.objects.count()"#;
        assert!(run_check(&QuerysetLen, src).is_empty());
    }

    // ── DJ011: MissingFExpression ─────────────────────────────────────────

    #[test]
    fn dj011_triggers_on_self_field_binop_assign_in_model() {
        // self.score = self.score + 1  inside a Model method
        let src = r#"
class Player(Model):
    def increment(self):
        self.score = self.score + 1
"#;
        assert!(run_check(&MissingFExpression, src).contains(&"DJ011".to_string()));
    }

    #[test]
    fn dj011_triggers_on_aug_assign_in_model() {
        // self.score += 1  inside a Model method
        let src = r#"
class Player(Model):
    def increment(self):
        self.score += 1
"#;
        assert!(run_check(&MissingFExpression, src).contains(&"DJ011".to_string()));
    }

    /// Outside a Model class the pattern is not a race condition risk.
    #[test]
    fn dj011_no_trigger_outside_model() {
        let src = r#"
class Helper:
    def run(self):
        self.count += 1
"#;
        assert!(run_check(&MissingFExpression, src).is_empty());
    }

    // ── DJ014: RawSqlInjection ────────────────────────────────────────────

    #[test]
    fn dj014_triggers_on_execute_with_fstring() {
        let src = r#"cursor.execute(f"SELECT * FROM t WHERE id = {user_id}")"#;
        assert!(run_check(&RawSqlInjection, src).contains(&"DJ014".to_string()));
    }

    #[test]
    fn dj014_triggers_on_execute_with_format() {
        let src = r#"cursor.execute("SELECT * FROM t WHERE id = {}".format(user_id))"#;
        assert!(run_check(&RawSqlInjection, src).contains(&"DJ014".to_string()));
    }

    #[test]
    fn dj014_triggers_on_execute_with_percent_format() {
        let src = r#"cursor.execute("SELECT * FROM t WHERE id = %s" % user_id)"#;
        assert!(run_check(&RawSqlInjection, src).contains(&"DJ014".to_string()));
    }

    #[test]
    fn dj014_triggers_on_raw_with_fstring() {
        let src = r#"MyModel.objects.raw(f"SELECT * FROM t WHERE id = {user_id}")"#;
        assert!(run_check(&RawSqlInjection, src).contains(&"DJ014".to_string()));
    }

    #[test]
    fn dj014_no_trigger_on_parameterised_execute() {
        // Parameterised queries pass values separately — not string interpolation.
        let src = r#"cursor.execute("SELECT * FROM t WHERE id = %s", [user_id])"#;
        assert!(run_check(&RawSqlInjection, src).is_empty());
    }

    /// Migration files contain developer-authored SQL, not user input.
    #[test]
    fn dj014_skipped_in_migrations() {
        let src = r#"cursor.execute(f"ALTER TABLE {table} ADD COLUMN x int")"#;
        assert!(
            run_check_with_filename(&RawSqlInjection, src, "myapp/migrations/0001_initial.py")
                .is_empty()
        );
    }

    // ── DJ015: DefaultMetaOrdering ────────────────────────────────────────

    #[test]
    fn dj015_triggers_on_meta_ordering() {
        let src = r#"
class Article(Model):
    class Meta:
        ordering = ["-created_at"]
"#;
        assert!(run_check(&DefaultMetaOrdering, src).contains(&"DJ015".to_string()));
    }

    #[test]
    fn dj015_no_trigger_without_ordering() {
        let src = r#"
class Article(Model):
    class Meta:
        verbose_name = "article"
"#;
        assert!(run_check(&DefaultMetaOrdering, src).is_empty());
    }

    // ── DJ017: CsrfExempt ─────────────────────────────────────────────────

    #[test]
    fn dj017_triggers_on_csrf_exempt_view() {
        let src = r#"
@csrf_exempt
def my_view(request):
    pass
"#;
        assert!(run_check(&CsrfExempt, src).contains(&"DJ017".to_string()));
    }

    #[test]
    fn dj017_no_trigger_on_normal_view() {
        let src = r#"
def my_view(request):
    pass
"#;
        assert!(run_check(&CsrfExempt, src).is_empty());
    }

    /// Webhook handlers legitimately need csrf_exempt — the check must be
    /// suppressed when the function name contains "webhook".
    #[test]
    fn dj017_no_trigger_on_webhook_function() {
        let src = r#"
@csrf_exempt
def stripe_webhook(request):
    pass
"#;
        assert!(run_check(&CsrfExempt, src).is_empty());
    }

    /// The check is suppressed entirely for files that look like webhook/api
    /// handlers (e.g. webhooks.py, api.py).
    #[test]
    fn dj017_skipped_in_webhook_file() {
        let src = r#"
@csrf_exempt
def handle(request):
    pass
"#;
        assert!(run_check_with_filename(&CsrfExempt, src, "payments/webhooks.py").is_empty());
    }

    // ── DJ018: RequestPostBoolCheck ───────────────────────────────────────

    #[test]
    fn dj018_triggers_on_if_request_post() {
        let src = r#"
if request.POST:
    pass
"#;
        assert!(run_check(&RequestPostBoolCheck, src).contains(&"DJ018".to_string()));
    }

    #[test]
    fn dj018_triggers_on_if_not_request_post() {
        let src = r#"
if not request.POST:
    pass
"#;
        assert!(run_check(&RequestPostBoolCheck, src).contains(&"DJ018".to_string()));
    }

    #[test]
    fn dj018_no_trigger_on_method_check() {
        let src = r#"
if request.method == "POST":
    pass
"#;
        assert!(run_check(&RequestPostBoolCheck, src).is_empty());
    }

    // ── DJ019: CountGreaterThanZero ───────────────────────────────────────

    #[test]
    fn dj019_triggers_on_count_gt_zero() {
        let src = r#"if MyModel.objects.count() > 0: pass"#;
        assert!(run_check(&CountGreaterThanZero, src).contains(&"DJ019".to_string()));
    }

    #[test]
    fn dj019_triggers_on_count_neq_zero() {
        let src = r#"if MyModel.objects.count() != 0: pass"#;
        assert!(run_check(&CountGreaterThanZero, src).contains(&"DJ019".to_string()));
    }

    #[test]
    fn dj019_triggers_on_zero_lt_count() {
        let src = r#"if 0 < MyModel.objects.count(): pass"#;
        assert!(run_check(&CountGreaterThanZero, src).contains(&"DJ019".to_string()));
    }

    #[test]
    fn dj019_no_trigger_on_exists() {
        let src = r#"if MyModel.objects.filter(active=True).exists(): pass"#;
        assert!(run_check(&CountGreaterThanZero, src).is_empty());
    }

    #[test]
    fn dj019_no_trigger_on_count_gt_one() {
        // count() > 1 is not equivalent to .exists() — do not flag it.
        let src = r#"if MyModel.objects.count() > 1: pass"#;
        assert!(run_check(&CountGreaterThanZero, src).is_empty());
    }

    // ── DJ020: SelectRelatedNoArgs ────────────────────────────────────────

    #[test]
    fn dj020_triggers_on_select_related_no_args() {
        let src = r#"qs = MyModel.objects.select_related()"#;
        assert!(run_check(&SelectRelatedNoArgs, src).contains(&"DJ020".to_string()));
    }

    #[test]
    fn dj020_no_trigger_when_field_specified() {
        let src = r#"qs = MyModel.objects.select_related("author")"#;
        assert!(run_check(&SelectRelatedNoArgs, src).is_empty());
    }

    // ── DJ021: FloatFieldForMoney ─────────────────────────────────────────

    #[test]
    fn dj021_triggers_on_float_field_named_price() {
        let src = r#"
class Order(Model):
    price = models.FloatField()
"#;
        assert!(run_check(&FloatFieldForMoney, src).contains(&"DJ021".to_string()));
    }

    #[test]
    fn dj021_triggers_on_float_field_named_amount() {
        let src = r#"
class Invoice(Model):
    amount = models.FloatField()
"#;
        assert!(run_check(&FloatFieldForMoney, src).contains(&"DJ021".to_string()));
    }

    #[test]
    fn dj021_no_trigger_on_float_field_non_money_name() {
        // "latitude" does not contain a money keyword — must not be flagged.
        let src = r#"
class Location(Model):
    latitude = models.FloatField()
"#;
        assert!(run_check(&FloatFieldForMoney, src).is_empty());
    }

    #[test]
    fn dj021_no_trigger_on_decimal_field_for_money() {
        let src = r#"
class Order(Model):
    price = models.DecimalField(max_digits=10, decimal_places=2)
"#;
        assert!(run_check(&FloatFieldForMoney, src).is_empty());
    }

    // ── DJ022: MutableDefaultJsonField ────────────────────────────────────

    #[test]
    fn dj022_triggers_on_jsonfield_with_dict_default() {
        let src = r#"data = models.JSONField(default={})"#;
        assert!(run_check(&MutableDefaultJsonField, src).contains(&"DJ022".to_string()));
    }

    #[test]
    fn dj022_triggers_on_jsonfield_with_list_default() {
        let src = r#"tags = models.JSONField(default=[])"#;
        assert!(run_check(&MutableDefaultJsonField, src).contains(&"DJ022".to_string()));
    }

    #[test]
    fn dj022_no_trigger_when_using_callable_default() {
        // default=dict is the recommended pattern — must not be flagged.
        let src = r#"data = models.JSONField(default=dict)"#;
        assert!(run_check(&MutableDefaultJsonField, src).is_empty());
    }

    #[test]
    fn dj022_no_trigger_on_jsonfield_without_default() {
        let src = r#"data = models.JSONField(null=True)"#;
        assert!(run_check(&MutableDefaultJsonField, src).is_empty());
    }

    // ── DJ023: SignalWithoutDispatchUid ───────────────────────────────────

    #[test]
    fn dj023_triggers_on_receiver_without_dispatch_uid() {
        let src = r#"
@receiver(post_save, sender=User)
def handle_user_save(sender, **kwargs):
    pass
"#;
        assert!(run_check(&SignalWithoutDispatchUid, src).contains(&"DJ023".to_string()));
    }

    #[test]
    fn dj023_no_trigger_when_dispatch_uid_present() {
        let src = r#"
@receiver(post_save, sender=User, dispatch_uid="handle_user_save")
def handle_user_save(sender, **kwargs):
    pass
"#;
        assert!(run_check(&SignalWithoutDispatchUid, src).is_empty());
    }

    #[test]
    fn dj023_triggers_on_signal_connect_without_dispatch_uid() {
        let src = r#"post_save.connect(handle_save)"#;
        assert!(run_check(&SignalWithoutDispatchUid, src).contains(&"DJ023".to_string()));
    }

    #[test]
    fn dj023_no_trigger_on_connect_with_dispatch_uid() {
        let src = r#"post_save.connect(handle_save, dispatch_uid="my_uid")"#;
        assert!(run_check(&SignalWithoutDispatchUid, src).is_empty());
    }

    // ── DJ024: UniqueTogetherDeprecated ───────────────────────────────────

    #[test]
    fn dj024_triggers_on_unique_together_in_meta() {
        let src = r#"
class MyModel(Model):
    class Meta:
        unique_together = [("field_a", "field_b")]
"#;
        assert!(run_check(&UniqueTogetherDeprecated, src).contains(&"DJ024".to_string()));
    }

    #[test]
    fn dj024_no_trigger_without_unique_together() {
        let src = r#"
class MyModel(Model):
    class Meta:
        verbose_name = "my model"
"#;
        assert!(run_check(&UniqueTogetherDeprecated, src).is_empty());
    }

    /// unique_together outside a Model Meta must not be flagged.
    #[test]
    fn dj024_no_trigger_on_non_model_class() {
        let src = r#"
class SomeConfig:
    unique_together = [("a", "b")]
"#;
        assert!(run_check(&UniqueTogetherDeprecated, src).is_empty());
    }

    // ── DJ025: IndexTogetherDeprecated ────────────────────────────────────

    #[test]
    fn dj025_triggers_on_index_together_in_meta() {
        let src = r#"
class Article(Model):
    class Meta:
        index_together = [["pub_date", "headline"]]
"#;
        assert!(run_check(&IndexTogetherDeprecated, src).contains(&"DJ025".to_string()));
    }

    #[test]
    fn dj025_no_trigger_without_index_together() {
        let src = r#"
class Article(Model):
    class Meta:
        indexes = []
"#;
        assert!(run_check(&IndexTogetherDeprecated, src).is_empty());
    }

    // ── DJ026: SaveCreateInLoop ───────────────────────────────────────────

    #[test]
    fn dj026_triggers_on_save_in_for_loop() {
        let src = r#"
for item in items:
    item.save()
"#;
        assert!(run_check(&SaveCreateInLoop, src).contains(&"DJ026".to_string()));
    }

    #[test]
    fn dj026_triggers_on_objects_create_in_for_loop() {
        let src = r#"
for row in rows:
    MyModel.objects.create(name=row["name"])
"#;
        assert!(run_check(&SaveCreateInLoop, src).contains(&"DJ026".to_string()));
    }

    /// The early-exit pattern (save then immediately return/break) executes the
    /// save at most once per loop and must NOT be flagged.
    #[test]
    fn dj026_no_trigger_on_save_with_early_exit() {
        let src = r#"
for item in items:
    if item.needs_update:
        item.save()
        return
"#;
        assert!(run_check(&SaveCreateInLoop, src).is_empty());
    }

    #[test]
    fn dj026_no_trigger_on_save_with_break() {
        let src = r#"
for item in items:
    if item.needs_update:
        item.save()
        break
"#;
        assert!(run_check(&SaveCreateInLoop, src).is_empty());
    }

    /// save() calls inside try/except blocks are error-handling patterns that
    /// must not be flagged.
    #[test]
    fn dj026_no_trigger_on_save_in_try_block() {
        let src = r#"
for item in items:
    try:
        item.save()
    except Exception:
        pass
"#;
        assert!(run_check(&SaveCreateInLoop, src).is_empty());
    }

    /// Seed / fixture files intentionally create records in loops.
    #[test]
    fn dj026_skipped_in_seed_file() {
        let src = r#"
for row in data:
    MyModel.objects.create(name=row["name"])
"#;
        assert!(run_check_with_filename(&SaveCreateInLoop, src, "db/seed_data.py").is_empty());
    }

    /// The check is suppressed inside test directories where per-row .save()
    /// calls are a common and acceptable fixture-building pattern.
    #[test]
    fn dj026_skipped_in_tests_directory() {
        let src = r#"
for item in items:
    item.save()
"#;
        assert!(
            run_check_with_filename(&SaveCreateInLoop, src, "myapp/tests/test_views.py").is_empty()
        );
    }
}
