//! # Contract: counting MetReader decorator
//!
//! Wraps a production MetReader and increments shared IoCallCounters on
//! every inspect/build_index/decode without altering results.
//!
use std::path::Path;
use std::sync::Arc;

use crate::io::metrics::IoCallCounters;
use crate::io::reader::{
    DecodeError, DecodeRequest, DecodedField, MetReader, SourceIndex, SourceMetadata,
};

/// [`MetReader`] wrapper that records production I/O calls.
pub struct CountingReader {
    inner: Box<dyn MetReader>,
    counters: Arc<IoCallCounters>,
}

impl CountingReader {
    /// Wraps an existing reader.
    #[must_use]
    pub fn new(inner: Box<dyn MetReader>, counters: Arc<IoCallCounters>) -> Self {
        Self { inner, counters }
    }
}

impl MetReader for CountingReader {
    fn inspect(&self, file: &Path) -> Result<SourceMetadata, DecodeError> {
        self.counters.record_inspect();
        self.inner.inspect(file)
    }

    fn build_index(&self, file: &Path) -> Result<Box<dyn SourceIndex>, DecodeError> {
        self.counters.record_build_index();
        self.inner.build_index(file)
    }

    fn decode(
        &self,
        file: &Path,
        index: &dyn SourceIndex,
        request: &DecodeRequest,
    ) -> Result<DecodedField, DecodeError> {
        self.counters.record_decode();
        self.inner.decode(file, index, request)
    }
}
