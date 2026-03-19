pub mod bridge;
mod checks;
mod config;

use colored::Colorize;
use std::collections::HashMap;
use thorn_api::{AppGraph, AstCheck, Diagnostic, GraphCheck, InitResult, Plugin, PluginParam};

/// Bundle format produced by `python -m thorn_django`:
/// `{ "graph": {...}, "diagnostics": [...] }`
#[derive(serde::Deserialize)]
struct GraphBundle {
    graph: AppGraph,
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
}

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

    fn cli_params(&self) -> Vec<PluginParam> {
        vec![
            PluginParam {
                name: "settings",
                help: "Django settings module (e.g. myproject.settings.production)",
                takes_value: true,
            },
            PluginParam {
                name: "graph-file",
                help: "Path to a pre-generated model graph JSON file",
                takes_value: true,
            },
        ]
    }

    /// Discover the Django model graph and run dynamic validation.
    ///
    /// Priority for settings:
    /// 1. `--django-settings` CLI arg
    /// 2. `THORN_DJANGO_SETTINGS` env var
    /// 3. `[tool.thorn-django] settings` in pyproject.toml
    /// 4. `DJANGO_SETTINGS_MODULE` env var
    ///
    /// Priority for graph source:
    /// 1. `--django-graph-file` CLI arg
    /// 2. `THORN_GRAPH_FILE` env var
    /// 3. `graph_file` key in `[tool.thorn-django]` pyproject.toml
    /// 4. Auto-discovered `.thorn/graph.json` inside `project_dir`
    /// 5. PyO3 in-process live extraction
    /// 6. Subprocess: `python3 -m thorn_django --settings <module>`
    fn initialize(
        &mut self,
        project_dir: &std::path::Path,
        toml_content: &str,
        cli_args: &HashMap<String, String>,
    ) -> InitResult {
        // ── 1. Resolve settings module ──────────────────────────────────────
        // CLI arg takes highest priority
        let settings_module = cli_args
            .get("settings")
            .cloned()
            .or_else(|| config::read_django_settings(toml_content));

        // ── 2. Discover graph file path ─────────────────────────────────────
        // Priority: CLI arg > env var > [tool.thorn-django].graph_file > auto-discover
        let graph_file = cli_args
            .get("graph-file")
            .map(|p| project_dir.join(p))
            .or_else(|| {
                std::env::var("THORN_GRAPH_FILE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(std::path::PathBuf::from)
            })
            .or_else(|| config::read_graph_file_path(toml_content).map(|p| project_dir.join(p)))
            .or_else(|| {
                let auto = project_dir.join(".thorn/graph.json");
                if auto.exists() {
                    Some(auto)
                } else {
                    None
                }
            });

        // ── 3. Try loading from graph file ──────────────────────────────────
        if let Some(ref graph_path) = graph_file {
            match std::fs::read_to_string(graph_path) {
                Ok(json_str) => {
                    // Try bundle format: { graph, diagnostics }
                    if let Ok(bundle) = serde_json::from_str::<GraphBundle>(&json_str) {
                        eprintln!(
                            "{} Loaded {} models from {}",
                            "✓".green(),
                            bundle.graph.models.len(),
                            graph_path.display()
                        );
                        if !bundle.diagnostics.is_empty() {
                            eprintln!(
                                "{} Dynamic validation found {} issue{}",
                                "✓".green(),
                                bundle.diagnostics.len(),
                                if bundle.diagnostics.len() == 1 {
                                    ""
                                } else {
                                    "s"
                                }
                            );
                        }

                        // Staleness check: are any models/*.py newer than the graph file?
                        self.check_graph_staleness(project_dir, graph_path);

                        let mut diagnostics = bundle.diagnostics;
                        apply_dedup_and_filter(&mut diagnostics);

                        return InitResult {
                            graph: bundle.graph,
                            diagnostics,
                        };
                    }

                    // Try plain graph format: { models, ... }
                    if let Ok(graph) = serde_json::from_str::<AppGraph>(&json_str) {
                        eprintln!(
                            "{} Loaded {} models from {}",
                            "✓".green(),
                            graph.models.len(),
                            graph_path.display()
                        );

                        self.check_graph_staleness(project_dir, graph_path);

                        return InitResult {
                            graph,
                            diagnostics: vec![],
                        };
                    }

                    eprintln!("{} Failed to parse graph file", "✗".red());
                }
                Err(e) => {
                    eprintln!("{} Failed to read graph file: {e}", "✗".red());
                }
            }
        }

        // ── 4. No graph file — try live extraction ──────────────────────────
        if let Some(ref settings) = settings_module {
            // 4a. PyO3 in-process (fastest — if Django is importable)
            if let Ok((graph, mut dv)) = bridge::extract_and_validate(settings) {
                eprintln!(
                    "{} Loaded {} models via PyO3",
                    "✓".green(),
                    graph.models.len()
                );
                apply_dedup_and_filter(&mut dv);
                return InitResult {
                    graph,
                    diagnostics: dv,
                };
            }

            // 4b. Subprocess: python3 -m thorn_django --settings <module>
            let graph_target = project_dir.join(".thorn/graph.json");
            for python in &["python3", "python"] {
                let ok = std::process::Command::new(python)
                    .args(["-m", "thorn_django", "--settings", settings])
                    .current_dir(project_dir)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if ok && graph_target.exists() {
                    if let Ok(s) = std::fs::read_to_string(&graph_target) {
                        if let Ok(bundle) = serde_json::from_str::<GraphBundle>(&s) {
                            eprintln!(
                                "{} Loaded {} models via python subprocess",
                                "✓".green(),
                                bundle.graph.models.len()
                            );
                            let mut dv = bundle.diagnostics;
                            apply_dedup_and_filter(&mut dv);
                            return InitResult {
                                graph: bundle.graph,
                                diagnostics: dv,
                            };
                        }
                    }
                }
            }
        }

        // ── 5. Nothing worked ───────────────────────────────────────────────
        eprintln!(
            "{} No .thorn/graph.json and no Django environment found.\n  \
             Generate once: python -m thorn_django --settings myproject.settings\n  \
             Or in Docker:  docker compose exec app python -m thorn_django",
            "!".yellow(),
        );

        InitResult::default()
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
            Box::new(checks::ast::ModelFormUsesExclude), // DJ002
            Box::new(checks::ast::RawSqlUsage),          // DJ003
            Box::new(checks::ast::LocalsInRender),       // DJ004
            Box::new(checks::ast::ForeignKeyMissingOnDelete), // DJ006
            Box::new(checks::ast::ModelFormFieldsAll),   // DJ007
            Box::new(checks::ast::RandomOrderBy),        // DJ008
            Box::new(checks::ast::QuerysetBoolEval),     // DJ009
            Box::new(checks::ast::QuerysetLen),          // DJ010
            Box::new(checks::ast::MissingFExpression),   // DJ011
            Box::new(checks::ast::RawSqlInjection),      // DJ014
            Box::new(checks::ast::DefaultMetaOrdering),  // DJ015
            Box::new(checks::ast::CsrfExempt),           // DJ017
            Box::new(checks::ast::RequestPostBoolCheck), // DJ018
            Box::new(checks::ast::CountGreaterThanZero), // DJ019
            Box::new(checks::ast::SelectRelatedNoArgs),  // DJ020
            Box::new(checks::ast::FloatFieldForMoney),   // DJ021
            Box::new(checks::ast::MutableDefaultJsonField), // DJ022
            Box::new(checks::ast::SignalWithoutDispatchUid), // DJ023
            Box::new(checks::ast::UniqueTogetherDeprecated), // DJ024
            Box::new(checks::ast::IndexTogetherDeprecated), // DJ025
            Box::new(checks::ast::SaveCreateInLoop),     // DJ026
            Box::new(checks::ast::CeleryDelayInAtomic),  // DJ027
            Box::new(checks::ast::RedirectReverse),      // DJ028
            Box::new(checks::ast::UnfilteredDelete),     // DJ029
            Box::new(checks::ast::DRFAllowAnyPermission), // DJ030
            Box::new(checks::ast::DRFEmptyAuthClasses),  // DJ031
            Box::new(checks::ast::DjangoValidationErrorInDRF), // DJ032
            Box::new(checks::ast::DRFNoPaginationClass), // DJ033
            Box::new(checks::ast::TooManyArguments {
                max_args: cfg.max_function_args,
            }), // DJ034
            Box::new(checks::ast::TooManyReturnStatements {
                max_returns: cfg.max_return_statements,
            }), // DJ035
            Box::new(checks::ast::TooManyBranches {
                max_branches: cfg.max_branches,
            }), // DJ036
            Box::new(checks::ast::TooManyLocalVariables {
                max_locals: cfg.max_local_variables,
            }), // DJ037
            Box::new(checks::ast::TooManyStatements {
                max_statements: cfg.max_statements,
            }), // DJ038
            Box::new(checks::ast::ModelTooManyFields {
                max_fields: cfg.max_model_fields,
            }), // DJ039
            Box::new(checks::ast::TooManyMethods {
                max_methods: cfg.max_class_methods,
            }), // DJ040
            Box::new(checks::ast::DeeplyNestedCode {
                max_depth: cfg.max_nesting_depth,
            }), // DJ041
            // ── pylint-django compat ──────────────────────────────────────
            Box::new(checks::ast::ModelUnicodeNotCallable), // E5101
            Box::new(checks::ast::ModelHasUnicode),         // W5102
            Box::new(checks::ast::HardCodedAuthUser),       // E5141
            Box::new(checks::ast::ImportedAuthUser),        // E5142
            Box::new(checks::ast::HttpResponseWithJsonDumps), // R5101
            Box::new(checks::ast::HttpResponseWithContentTypeJson), // R5102
            Box::new(checks::ast::RedundantContentTypeForJsonResponse), // R5103
            Box::new(checks::ast::MissingBackwardsMigrationCallable), // W5197
            Box::new(checks::ast::NewDbFieldWithDefault),   // W5198
        ];

        if !self.has_graph {
            // No graph — run AST-only fallbacks for checks that have graph versions
            checks.push(Box::new(checks::ast::NullableStringField)); // DJ001 (graph: DJ103)
            checks.push(Box::new(checks::ast::ModelWithoutStrMethod)); // DJ005 (graph: DJ101)
        }

        if self.has_graph {
            // Cross-referencing checks — need graph to work
            checks.push(Box::new(checks::cross::InvalidFilterField)); // DJ201
            checks.push(Box::new(checks::cross::InvalidValuesField)); // DJ202
            checks.push(Box::new(checks::cross::InvalidManagerMethod)); // DJ203
            checks.push(Box::new(checks::cross::InvalidGetDisplay)); // DJ204
            checks.push(Box::new(checks::cross::SerializerFieldMismatch)); // DJ205
            checks.push(Box::new(checks::cross::WrongReverseAccessor)); // DJ206
            checks.push(Box::new(checks::cross::ForeignKeyIdAccess)); // DJ207
        }

        checks
    }

    fn project_checks(&self, project_dir: &std::path::Path, toml_content: &str) -> Vec<Diagnostic> {
        let settings_module = config::read_django_settings(toml_content).unwrap_or_else(|| {
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
            diagnostics.extend(checks::settings::check_settings(
                project_dir,
                &settings_module,
            ));
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
            Box::new(checks::graph::GraphModelMissingStr), // DJ101
            Box::new(checks::graph::DuplicateRelatedName), // DJ102
            Box::new(checks::graph::NullableStringFieldGraph), // DJ103
            Box::new(checks::graph::MissingReverseAccessor), // DJ104
        ]
    }
}

impl DjangoPlugin {
    /// Warn if any `models*.py` file inside `project_dir` is newer than `graph_path`.
    fn check_graph_staleness(&self, project_dir: &std::path::Path, graph_path: &std::path::Path) {
        let graph_modified = match std::fs::metadata(graph_path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return,
        };

        let has_newer = walkdir::WalkDir::new(project_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let p = e.path().to_string_lossy();
                p.ends_with(".py")
                    && (p.contains("models") || p.contains("model"))
                    && !p.contains("site-packages")
                    && !p.contains("/.venv/")
                    && !p.contains("migrations")
            })
            .any(|e| {
                e.metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| t > graph_modified)
                    .unwrap_or(false)
            });

        if has_newer {
            eprintln!(
                "{} .thorn/graph.json may be stale — model files have changed.\n  \
                 Regenerate with: docker compose exec app python -m thorn_django",
                "!".yellow(),
            );
        }
    }
}

/// Apply deduplication and third-party filtering rules to a set of dynamic diagnostics.
///
/// Rules applied:
/// - DV001 (runtime MRO __str__ check) supersedes DJ101 (graph-based __str__ check):
///   if any DV001 exists, DJ101 entries are dropped.
/// - DV diagnostics from site-packages / venv paths are dropped.
/// - DV diagnostics whose filename has no path separator and no `.py` suffix
///   (i.e. a bare module name like `"qualificationcheck.forms"`) are from
///   third-party packages whose source file could not be resolved — drop them.
fn apply_dedup_and_filter(diagnostics: &mut Vec<Diagnostic>) {
    // DV001 supersedes DJ101
    let has_dv001 = diagnostics.iter().any(|d| d.code == "DV001");
    if has_dv001 {
        diagnostics.retain(|d| d.code != "DJ101");
    }

    // Filter third-party / site-packages DV diagnostics
    diagnostics.retain(|d| {
        if d.code.starts_with("DV")
            && d.code != "DV-WARN"
            && d.code != "DV-ERR"
            && d.code != "DV-CRIT"
        {
            let f = &d.filename;
            if f.contains("site-packages") || f.contains("/venv/") || f.contains("/.venv/") {
                return false;
            }
            // Bare module-name filenames (no "/" and no ".py") are third-party
            if !f.contains('/')
                && !f.contains(".py")
                && f != "migrations"
                && f != "django.checks"
                && f != "settings"
            {
                return false;
            }
        }
        true
    });
}
