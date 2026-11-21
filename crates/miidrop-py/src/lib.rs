use pyo3::prelude::*;

#[pymodule]
fn miidrop(_m: &Bound<'_, PyModule>) -> PyResult<()> {
    Ok(())
}
