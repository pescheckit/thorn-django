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
    let django = py.import("django")?;
    django.call_method0("setup")?;
    Ok(())
}
