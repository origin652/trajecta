//! Safe controls for HDF5 behavior that is otherwise exposed only through C FFI.

use std::error::Error;
use std::fmt;

/// Failure returned when HDF5 rejects a process-control request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hdf5ControlError;

impl fmt::Display for Hdf5ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("failed to disable HDF5 automatic error printing")
    }
}

impl Error for Hdf5ControlError {}

/// Disables HDF5's automatic error-stack printer for the current thread.
///
/// HDF5 and netCDF operations continue to return their normal error codes. This
/// only prevents expected internal probes from writing directly to process
/// stderr before the caller can turn the result into a structured diagnostic.
pub fn disable_automatic_error_printing() -> Result<(), Hdf5ControlError> {
    // SAFETY: H5E_DEFAULT selects the current thread's default error stack. A
    // null callback and client pointer are the documented disable operation.
    let status = unsafe {
        hdf5_metno_sys::h5e::H5Eset_auto2(
            hdf5_metno_sys::h5e::H5E_DEFAULT,
            None,
            std::ptr::null_mut(),
        )
    };
    (status >= 0).then_some(()).ok_or(Hdf5ControlError)
}
