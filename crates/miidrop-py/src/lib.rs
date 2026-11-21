//! Python bindings for reading Mii records.

use ::miidrop::container;
use ::miidrop::keys::Keys;
use ::miidrop::mii::{self, FIELDS, RECORD_SIZE, STORED_SIZE, WRAPPED_SIZE};
use pyo3::exceptions::{PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict};

fn err(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

#[pyclass(module = "miidrop", frozen)]
#[derive(Clone)]
struct Record(mii::Record);

#[pymethods]
impl Record {
    /// Parses a 96-byte record or a 92-byte stored one.
    #[new]
    fn new(data: &[u8]) -> PyResult<Self> {
        match data.len() {
            STORED_SIZE => mii::Record::from_stored(data),
            _ => mii::Record::from_bytes(data),
        }
        .map(Record)
        .map_err(err)
    }

    fn __bytes__<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.0.as_bytes())
    }

    #[getter]
    fn name(&self) -> String {
        self.0.name()
    }

    #[getter]
    fn creator(&self) -> String {
        self.0.creator_name()
    }

    #[getter]
    fn mii_id(&self) -> u32 {
        self.0.mii_id()
    }

    #[getter]
    fn crc_valid(&self) -> bool {
        self.0.crc_valid()
    }

    #[getter]
    fn favourite(&self) -> bool {
        self.0.favourite()
    }

    #[getter]
    fn copying_allowed(&self) -> bool {
        self.0.copying_allowed()
    }

    #[getter]
    fn sharing_disabled(&self) -> bool {
        self.0.sharing_disabled()
    }

    /// (month, day), with zeros when unset.
    #[getter]
    fn birthday(&self) -> (u32, u32) {
        self.0.birthday()
    }

    #[getter]
    fn system_id(&self) -> String {
        hex(&self.0.system_id())
    }

    #[getter]
    fn creator_mac(&self) -> String {
        hex(&self.0.creator_mac())
    }

    /// Problems a receiving console would reject the record for.
    fn issues(&self) -> Vec<String> {
        self.0.validate().iter().map(|i| i.to_string()).collect()
    }

    fn field(&self, name: &str) -> PyResult<u32> {
        self.0
            .field(name)
            .ok_or_else(|| PyKeyError::new_err(name.to_string()))
    }

    /// Every appearance field by name.
    fn fields<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for f in FIELDS {
            d.set_item(f.name, self.0.get(f))?;
        }
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!("<Record {:?} {:08x}>", self.0.name(), self.0.mii_id())
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Reads a record from any supported form; wrapped records and containers need the key file's text.
#[pyfunction]
#[pyo3(signature = (data, keys=None))]
fn read(data: &[u8], keys: Option<&str>) -> PyResult<Record> {
    if data.len() == RECORD_SIZE || data.len() == STORED_SIZE {
        return Record::new(data);
    }
    let keys = keys.ok_or_else(|| err("this file is protected and needs the key file"))?;
    let keys = Keys::parse(keys).map_err(err)?;
    let wrapped: [u8; WRAPPED_SIZE] = if data.len() == WRAPPED_SIZE {
        data.try_into().unwrap()
    } else {
        container::parse(data).map_err(err)?
    };
    mii::unwrap(&keys.record, &wrapped).map(Record).map_err(err)
}

/// Checks the key file's text, returning whether it also holds the beacon key needed to host.
#[pyfunction]
fn check_keys(text: &str) -> PyResult<bool> {
    Keys::parse(text).map(|k| k.beacon.is_some()).map_err(err)
}

#[pymodule]
#[pyo3(name = "miidrop")]
fn miidrop_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Record>()?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(check_keys, m)?)?;
    m.add("RECORD_SIZE", RECORD_SIZE)?;
    Ok(())
}
