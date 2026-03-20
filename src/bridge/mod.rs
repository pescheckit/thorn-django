mod extract;
pub mod validate;

use pyo3::prelude::*;
use thorn_api::graph::AppGraph;
use thorn_api::Diagnostic;

pub fn extract_model_graph(settings_module: &str) -> Result<AppGraph, PyErr> {
    Python::with_gil(|py| {
        boot_django(py, settings_module)?;
        extract::extract_graph(py)
    })
}

pub fn extract_and_validate(settings_module: &str) -> Result<(AppGraph, Vec<Diagnostic>), PyErr> {
    Python::with_gil(|py| {
        boot_django(py, settings_module)?;
        let graph = extract::extract_graph(py)?;
        let diagnostics = validate::run_all_dynamic_checks(py)?;
        Ok((graph, diagnostics))
    })
}

fn boot_django(py: Python<'_>, settings_module: &str) -> PyResult<()> {
    let os = py.import("os")?;
    let environ = os.getattr("environ")?;
    environ.call_method1("setdefault", ("DJANGO_SETTINGS_MODULE", settings_module))?;

    // Suppress all logging before Django's setup() configures its loggers.
    // CRITICAL = 50 disables every level below it (DEBUG=10, INFO=20, WARNING=30, ERROR=40).
    let logging = py.import("logging")?;
    logging.call_method1("disable", (50i32,))?;

    // Redirect stderr to /dev/null so Django's early print-style warnings are
    // also silenced (e.g. deprecation notices written directly to stderr).
    let sys = py.import("sys")?;
    let builtins = py.import("builtins")?;
    let devnull = builtins.call_method1("open", ("/dev/null", "w"))?;
    sys.setattr("stderr", &devnull)?;

    let django = py.import("django")?;
    django.call_method0("setup")?;

    // After setup(), Django may have reconfigured individual loggers at lower
    // levels. Walk every logger it registered and silence them individually so
    // that re-enabling the global threshold (below) doesn't let them speak.
    if let Ok(manager) = logging.getattr("Logger").and_then(|l| l.getattr("manager")) {
        if let Ok(logger_dict) = manager.getattr("loggerDict") {
            if let Ok(keys) = logger_dict.call_method0("keys") {
                if let Ok(iter) = keys.try_iter() {
                    for name in iter.flatten() {
                        if let Ok(name_str) = name.extract::<String>() {
                            if let Ok(logger) = logging.call_method1("getLogger", (&name_str,)) {
                                let _ = logger.call_method1("setLevel", (50i32,));
                            }
                        }
                    }
                }
            }
        }
    }

    // Re-enable the global threshold so that our own Rust-side eprintln! output
    // is not accidentally filtered if anything checks the Python logging state.
    logging.call_method1("disable", (0i32,))?;

    // Restore the real stderr so that thorn's own error output reaches the user.
    let real_stderr = sys.getattr("__stderr__")?;
    sys.setattr("stderr", real_stderr)?;

    Ok(())
}
