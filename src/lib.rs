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
    settings_module: Option<String>,
}

impl DjangoPlugin {
    pub fn new() -> Self {
        Self {
            has_graph: false,
            settings_module: None,
        }
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

    fn initialize(
        &mut self,
        project_dir: &std::path::Path,
        toml_content: &str,
        cli_args: &HashMap<String, String>,
    ) -> InitResult {
        // ── 1. Resolve settings module ──────────────────────────────────────
        let settings_module = cli_args
            .get("settings")
            .cloned()
            .or_else(|| config::read_django_settings(toml_content));
        self.settings_module = settings_module.clone();

        // ── 2. Discover graph file path ─────────────────────────────────────
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
                        self.check_graph_staleness(project_dir, graph_path);
                        let mut diagnostics = bundle.diagnostics;
                        apply_dedup_and_filter(&mut diagnostics);
                        return InitResult {
                            graph: bundle.graph,
                            diagnostics,
                        };
                    }
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
        let cfg = config::read_django_config_from_cwd();
        let mut checks: Vec<Box<dyn AstCheck>> = vec![
            Box::new(checks::ast::ModelFormUsesExclude),
            Box::new(checks::ast::RawSqlUsage),
            Box::new(checks::ast::LocalsInRender),
            Box::new(checks::ast::ForeignKeyMissingOnDelete),
            Box::new(checks::ast::ModelFormFieldsAll),
            Box::new(checks::ast::RandomOrderBy),
            Box::new(checks::ast::QuerysetBoolEval),
            Box::new(checks::ast::QuerysetLen),
            Box::new(checks::ast::MissingFExpression),
            Box::new(checks::ast::RawSqlInjection),
            Box::new(checks::ast::DefaultMetaOrdering),
            Box::new(checks::ast::CsrfExempt),
            Box::new(checks::ast::RequestPostBoolCheck),
            Box::new(checks::ast::CountGreaterThanZero),
            Box::new(checks::ast::SelectRelatedNoArgs),
            Box::new(checks::ast::FloatFieldForMoney),
            Box::new(checks::ast::MutableDefaultJsonField),
            Box::new(checks::ast::SignalWithoutDispatchUid),
            Box::new(checks::ast::UniqueTogetherDeprecated),
            Box::new(checks::ast::IndexTogetherDeprecated),
            Box::new(checks::ast::SaveCreateInLoop),
            Box::new(checks::ast::CeleryDelayInAtomic),
            Box::new(checks::ast::RedirectReverse),
            Box::new(checks::ast::UnfilteredDelete),
            Box::new(checks::ast::DRFAllowAnyPermission),
            Box::new(checks::ast::DRFEmptyAuthClasses),
            Box::new(checks::ast::DjangoValidationErrorInDRF),
            Box::new(checks::ast::DRFNoPaginationClass),
            Box::new(checks::ast::TooManyArguments {
                max_args: cfg.max_function_args,
            }),
            Box::new(checks::ast::TooManyReturnStatements {
                max_returns: cfg.max_return_statements,
            }),
            Box::new(checks::ast::TooManyBranches {
                max_branches: cfg.max_branches,
            }),
            Box::new(checks::ast::TooManyLocalVariables {
                max_locals: cfg.max_local_variables,
            }),
            Box::new(checks::ast::TooManyStatements {
                max_statements: cfg.max_statements,
            }),
            Box::new(checks::ast::ModelTooManyFields {
                max_fields: cfg.max_model_fields,
            }),
            Box::new(checks::ast::TooManyMethods {
                max_methods: cfg.max_class_methods,
            }),
            Box::new(checks::ast::DeeplyNestedCode {
                max_depth: cfg.max_nesting_depth,
            }),
            Box::new(checks::ast::ModelUnicodeNotCallable),
            Box::new(checks::ast::ModelHasUnicode),
            Box::new(checks::ast::HardCodedAuthUser),
            Box::new(checks::ast::ImportedAuthUser),
            Box::new(checks::ast::HttpResponseWithJsonDumps),
            Box::new(checks::ast::HttpResponseWithContentTypeJson),
            Box::new(checks::ast::RedundantContentTypeForJsonResponse),
            Box::new(checks::ast::MissingBackwardsMigrationCallable),
            Box::new(checks::ast::NewDbFieldWithDefault),
        ];

        if !self.has_graph {
            checks.push(Box::new(checks::ast::NullableStringField));
            checks.push(Box::new(checks::ast::ModelWithoutStrMethod));
        }

        if self.has_graph {
            checks.push(Box::new(checks::cross::InvalidFilterField));
            checks.push(Box::new(checks::cross::InvalidValuesField));
            checks.push(Box::new(checks::cross::InvalidManagerMethod));
            checks.push(Box::new(checks::cross::InvalidGetDisplay));
            checks.push(Box::new(checks::cross::SerializerFieldMismatch));
            checks.push(Box::new(checks::cross::WrongReverseAccessor));
            checks.push(Box::new(checks::cross::ForeignKeyIdAccess));
        }

        checks
    }

    fn project_checks(&self, project_dir: &std::path::Path, toml_content: &str) -> Vec<Diagnostic> {
        let settings_module = self
            .settings_module
            .clone()
            .or_else(|| config::read_django_settings(toml_content))
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
            diagnostics.extend(checks::settings::check_settings(
                project_dir,
                &settings_module,
            ));
        }
        diagnostics.extend(checks::imports::check_imports(project_dir));
        diagnostics
    }

    fn read_config_excludes(&self, toml_content: &str) -> Vec<String> {
        config::read_django_excludes(toml_content)
    }

    fn graph_checks(&self) -> Vec<Box<dyn GraphCheck>> {
        vec![
            Box::new(checks::graph::GraphModelMissingStr),
            Box::new(checks::graph::DuplicateRelatedName),
            Box::new(checks::graph::NullableStringFieldGraph),
            Box::new(checks::graph::MissingReverseAccessor),
        ]
    }
}

impl DjangoPlugin {
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

fn apply_dedup_and_filter(diagnostics: &mut Vec<Diagnostic>) {
    let has_dv001 = diagnostics.iter().any(|d| d.code == "DV001");
    if has_dv001 {
        diagnostics.retain(|d| d.code != "DJ101");
    }
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
