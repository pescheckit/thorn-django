pub mod bridge;
mod checks;
mod config;

use thorn_api::{AppGraph, AstCheck, Diagnostic, GraphCheck, Plugin};

pub struct DjangoPlugin {
    has_graph: bool,
}

impl DjangoPlugin {
    pub fn new() -> Self {
        Self { has_graph: false }
    }
}

impl Default for DjangoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for DjangoPlugin {
    fn name(&self) -> &'static str {
        "django"
    }

    fn prefix(&self) -> &'static str {
        "DJ"
    }

    fn on_graph_ready(&mut self, graph: &AppGraph) {
        self.has_graph = !graph.models.is_empty();
    }

    fn ast_checks(&self) -> Vec<Box<dyn AstCheck>> {
        // Read thresholds from pyproject.toml (walks up from CWD).  Defaults
        // are identical to the original hardcoded values, so projects without a
        // [tool.thorn-django] section are unaffected.
        let cfg = config::read_django_config_from_cwd();

        let mut checks: Vec<Box<dyn AstCheck>> = vec![
            // ── Always run (no graph equivalent) ──────────────────────────
            Box::new(checks::ast::ModelFormUsesExclude),      // DJ002
            Box::new(checks::ast::RawSqlUsage),               // DJ003
            Box::new(checks::ast::LocalsInRender),            // DJ004
            Box::new(checks::ast::ForeignKeyMissingOnDelete), // DJ006
            Box::new(checks::ast::ModelFormFieldsAll),         // DJ007
            Box::new(checks::ast::RandomOrderBy),              // DJ008
            Box::new(checks::ast::QuerysetBoolEval),           // DJ009
            Box::new(checks::ast::QuerysetLen),                // DJ010
            Box::new(checks::ast::MissingFExpression),         // DJ011
            Box::new(checks::ast::RawSqlInjection),            // DJ014
            Box::new(checks::ast::DefaultMetaOrdering),        // DJ015
            Box::new(checks::ast::CsrfExempt),                // DJ017
            Box::new(checks::ast::RequestPostBoolCheck),       // DJ018
            Box::new(checks::ast::CountGreaterThanZero),       // DJ019
            Box::new(checks::ast::SelectRelatedNoArgs),        // DJ020
            Box::new(checks::ast::FloatFieldForMoney),         // DJ021
            Box::new(checks::ast::MutableDefaultJsonField),    // DJ022
            Box::new(checks::ast::SignalWithoutDispatchUid),   // DJ023
            Box::new(checks::ast::UniqueTogetherDeprecated),   // DJ024
            Box::new(checks::ast::IndexTogetherDeprecated),    // DJ025
            Box::new(checks::ast::SaveCreateInLoop),           // DJ026
            Box::new(checks::ast::CeleryDelayInAtomic),        // DJ027
            Box::new(checks::ast::RedirectReverse),            // DJ028
            Box::new(checks::ast::UnfilteredDelete),           // DJ029
            Box::new(checks::ast::DRFAllowAnyPermission),      // DJ030
            Box::new(checks::ast::DRFEmptyAuthClasses),        // DJ031
            Box::new(checks::ast::DjangoValidationErrorInDRF), // DJ032
            Box::new(checks::ast::DRFNoPaginationClass),       // DJ033
            Box::new(checks::ast::TooManyArguments       { max_args:       cfg.max_function_args }),      // DJ034
            Box::new(checks::ast::TooManyReturnStatements { max_returns:   cfg.max_return_statements }),  // DJ035
            Box::new(checks::ast::TooManyBranches        { max_branches:   cfg.max_branches }),           // DJ036
            Box::new(checks::ast::TooManyLocalVariables  { max_locals:     cfg.max_local_variables }),    // DJ037
            Box::new(checks::ast::TooManyStatements      { max_statements: cfg.max_statements }),         // DJ038
            Box::new(checks::ast::ModelTooManyFields     { max_fields:     cfg.max_model_fields }),       // DJ039
            Box::new(checks::ast::TooManyMethods         { max_methods:    cfg.max_class_methods }),      // DJ040
            Box::new(checks::ast::DeeplyNestedCode       { max_depth:      cfg.max_nesting_depth }),      // DJ041
            // ── pylint-django compat ──────────────────────────────────────
            Box::new(checks::ast::ModelUnicodeNotCallable),    // E5101
            Box::new(checks::ast::ModelHasUnicode),            // W5102
            Box::new(checks::ast::HardCodedAuthUser),          // E5141
            Box::new(checks::ast::ImportedAuthUser),           // E5142
            Box::new(checks::ast::HttpResponseWithJsonDumps),  // R5101
            Box::new(checks::ast::HttpResponseWithContentTypeJson), // R5102
            Box::new(checks::ast::RedundantContentTypeForJsonResponse), // R5103
            Box::new(checks::ast::MissingBackwardsMigrationCallable), // W5197
            Box::new(checks::ast::NewDbFieldWithDefault),      // W5198
        ];

        if !self.has_graph {
            // No graph — run AST-only fallbacks for checks that have graph versions
            checks.push(Box::new(checks::ast::NullableStringField));   // DJ001 (graph: DJ103)
            checks.push(Box::new(checks::ast::ModelWithoutStrMethod)); // DJ005 (graph: DJ101)
        }

        if self.has_graph {
            // Cross-referencing checks — need graph to work
            checks.push(Box::new(checks::cross::InvalidFilterField));      // DJ201
            checks.push(Box::new(checks::cross::InvalidValuesField));      // DJ202
            checks.push(Box::new(checks::cross::InvalidManagerMethod));    // DJ203
            checks.push(Box::new(checks::cross::InvalidGetDisplay));       // DJ204
            checks.push(Box::new(checks::cross::SerializerFieldMismatch)); // DJ205
            checks.push(Box::new(checks::cross::WrongReverseAccessor));    // DJ206
            checks.push(Box::new(checks::cross::ForeignKeyIdAccess));      // DJ207
        }

        checks
    }

    fn project_checks(&self, project_dir: &std::path::Path, toml_content: &str) -> Vec<Diagnostic> {
        let settings_module = config::read_django_settings(toml_content)
            .unwrap_or_else(|| {
                for candidate in &["settings", "config.settings", "conf.settings"] {
                    let path = project_dir.join(candidate.replace('.', "/") + ".py");
                    if path.exists() {
                        return candidate.to_string();
                    }
                }
                String::new()
            });

        let mut diagnostics = Vec::new();

        if !settings_module.is_empty() {
            diagnostics.extend(checks::settings::check_settings(project_dir, &settings_module));
        }

        // Cross-file import graph checks (DJ042, DJ043)
        diagnostics.extend(checks::imports::check_imports(project_dir));

        diagnostics
    }

    fn read_config_excludes(&self, toml_content: &str) -> Vec<String> {
        config::read_django_excludes(toml_content)
    }

    fn graph_checks(&self) -> Vec<Box<dyn GraphCheck>> {
        vec![
            Box::new(checks::graph::GraphModelMissingStr),     // DJ101
            Box::new(checks::graph::DuplicateRelatedName),     // DJ102
            Box::new(checks::graph::NullableStringFieldGraph), // DJ103
            Box::new(checks::graph::MissingReverseAccessor),   // DJ104
        ]
    }
}
