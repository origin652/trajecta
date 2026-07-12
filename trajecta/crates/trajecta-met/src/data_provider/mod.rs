//! # Contract: meteorological data providers
//!
//! Providers locate already-present immutable datasets. v0 provides local
//! lookup only; downloading is a future extension behind this boundary.

use std::collections::BTreeMap;
use std::path::PathBuf;

use trajecta_case::model::meteorology::DatasetRef;

/// Local root and immutable lockfile for one logical dataset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatasetLocation {
    /// Local dataset root.
    pub root: PathBuf,
    /// Local immutable dataset lockfile.
    pub lockfile: PathBuf,
}

/// Format-neutral dataset-location provider.
pub trait DataProvider: Send + Sync {
    /// Locates one logical dataset without downloading content.
    fn locate(&self, dataset: &DatasetRef) -> Result<DatasetLocation, ProviderError>;
}

/// v0 provider backed by explicit local RunProfile bindings.
#[derive(Clone, Debug, Default)]
pub struct LocalDataProvider {
    locations: BTreeMap<DatasetRef, DatasetLocation>,
}

impl LocalDataProvider {
    /// Creates an empty provider.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            locations: BTreeMap::new(),
        }
    }

    /// Registers one unique logical dataset.
    pub fn register(
        &mut self,
        dataset: DatasetRef,
        location: DatasetLocation,
    ) -> Result<(), ProviderError> {
        if self.locations.contains_key(&dataset) {
            return Err(ProviderError::DuplicateDataset(dataset));
        }
        self.locations.insert(dataset, location);
        Ok(())
    }
}

impl DataProvider for LocalDataProvider {
    fn locate(&self, dataset: &DatasetRef) -> Result<DatasetLocation, ProviderError> {
        self.locations
            .get(dataset)
            .cloned()
            .ok_or_else(|| ProviderError::UnknownDataset(dataset.clone()))
    }
}

/// Dataset-location failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderError {
    /// No binding exists for a logical dataset.
    UnknownDataset(DatasetRef),
    /// A logical dataset was bound more than once.
    DuplicateDataset(DatasetRef),
    /// A future remote provider operation is unavailable in v0.
    RemoteUnavailable,
}
