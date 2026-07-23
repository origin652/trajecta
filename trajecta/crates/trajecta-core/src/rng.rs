//! # Contract: order-independent deterministic sampling
//!
//! Random values are keyed by seed, stable population and lifecycle-event
//! digests, particle ordinal, sampling dimension, and draw index. They never
//! depend on thread, chunk, storage order, or a process-global random stream.

use sha2::{Digest, Sha256};

use crate::particle::ParticleId;

/// Frozen counter-based RNG identifier.
pub const COUNTER_RNG_ALGORITHM_ID: &str = "philox4x32-10/v1";

/// Release birth-time random dimension.
pub const RELEASE_BIRTH_TIME_DIMENSION: u32 = 0;
/// Release horizontal component-selection random dimension.
pub const RELEASE_HORIZONTAL_COMPONENT_DIMENSION: u32 = 1;
/// First release horizontal within-component coordinate dimension.
pub const RELEASE_HORIZONTAL_U_DIMENSION: u32 = 2;
/// Second release horizontal within-component coordinate dimension.
pub const RELEASE_HORIZONTAL_V_DIMENSION: u32 = 3;
/// Release event-native vertical coordinate dimension.
pub const RELEASE_VERTICAL_DIMENSION: u32 = 4;

/// Frozen deterministic low-discrepancy identifier.
pub const LOW_DISCREPANCY_ALGORITHM_ID: &str = "halton-primes/v1";

/// Stable 64-bit digest of a string identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StableRandomId(pub u64);

impl StableRandomId {
    /// Hashes an exact UTF-8 identifier with the frozen SHA-256 truncation rule.
    #[must_use]
    pub fn from_text(value: &str) -> Self {
        let digest = Sha256::digest(value.as_bytes());
        let mut bytes = [0_u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        Self(u64::from_be_bytes(bytes))
    }
}

/// Complete key for one counter-based random value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RandomKey {
    /// User-provided or manifest-generated run seed.
    pub seed: u64,
    /// Stable digest of the population ID.
    pub population: StableRandomId,
    /// Stable digest of release event or domain-fill lifecycle event identity.
    pub lifecycle_event: StableRandomId,
    /// Stable particle identity or pre-allocation ordinal.
    pub particle: ParticleId,
    /// Independent sampling dimension.
    pub sampling_dimension: u32,
    /// Repeated draw index within that dimension.
    pub draw_index: u32,
}

/// Counter-based random-number generator independent of execution order.
#[derive(Clone, Copy, Debug, Default)]
pub struct CounterRng;

impl CounterRng {
    /// Produces one deterministic 64-bit value for an exact key.
    #[must_use]
    pub fn sample_u64(key: RandomKey) -> u64 {
        let (counter, philox_key) = derive_philox_input(key);
        let words = philox4x32_10(counter, philox_key);
        (u64::from(words[0]) << 32) | u64::from(words[1])
    }

    /// Produces one deterministic value in the half-open interval `[0, 1)`.
    #[must_use]
    pub fn sample_unit(key: RandomKey) -> f64 {
        const SCALE: f64 = 1.0 / ((1_u64 << 53) as f64);
        ((Self::sample_u64(key) >> 11) as f64) * SCALE
    }
}

/// Deterministic unshifted Halton sampler for geometric fixtures and strata.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowDiscrepancySampler;

impl LowDiscrepancySampler {
    /// Produces a deterministic point in up to sixteen dimensions.
    ///
    /// Index zero maps to the first non-zero Halton point. Seeded production
    /// randomization must be applied explicitly with [`CounterRng`].
    pub fn sample(index: u64, dimensions: usize) -> Result<Vec<f64>, RngError> {
        const PRIMES: [u32; 16] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53];
        if dimensions == 0 || dimensions > PRIMES.len() {
            return Err(RngError::UnsupportedDimensions(dimensions));
        }
        let sequence_index = index.checked_add(1).ok_or(RngError::IndexOverflow)?;
        Ok(PRIMES[..dimensions]
            .iter()
            .map(|base| radical_inverse(sequence_index, *base))
            .collect())
    }
}

fn derive_philox_input(key: RandomKey) -> ([u32; 4], [u32; 2]) {
    let mut hasher = Sha256::new();
    hasher.update(b"trajecta/philox4x32-10/v1\0");
    hasher.update(key.seed.to_be_bytes());
    hasher.update(key.population.0.to_be_bytes());
    hasher.update(key.lifecycle_event.0.to_be_bytes());
    hasher.update(key.particle.0.to_be_bytes());
    hasher.update(key.sampling_dimension.to_be_bytes());
    hasher.update(key.draw_index.to_be_bytes());
    let digest = hasher.finalize();
    let mut words = [0_u32; 6];
    for (index, word) in words.iter_mut().enumerate() {
        let offset = index * 4;
        *word = u32::from_be_bytes([
            digest[offset],
            digest[offset + 1],
            digest[offset + 2],
            digest[offset + 3],
        ]);
    }
    (
        [words[0], words[1], words[2], words[3]],
        [words[4], words[5]],
    )
}

fn philox4x32_10(mut counter: [u32; 4], mut key: [u32; 2]) -> [u32; 4] {
    const MULTIPLIER_0: u32 = 0xD251_1F53;
    const MULTIPLIER_1: u32 = 0xCD9E_8D57;
    const WEYL_0: u32 = 0x9E37_79B9;
    const WEYL_1: u32 = 0xBB67_AE85;

    for round in 0..10 {
        let product_0 = u64::from(MULTIPLIER_0) * u64::from(counter[0]);
        let product_1 = u64::from(MULTIPLIER_1) * u64::from(counter[2]);
        let high_0 = (product_0 >> 32) as u32;
        let low_0 = product_0 as u32;
        let high_1 = (product_1 >> 32) as u32;
        let low_1 = product_1 as u32;
        counter = [
            high_1 ^ counter[1] ^ key[0],
            low_1,
            high_0 ^ counter[3] ^ key[1],
            low_0,
        ];
        if round != 9 {
            key[0] = key[0].wrapping_add(WEYL_0);
            key[1] = key[1].wrapping_add(WEYL_1);
        }
    }
    counter
}

fn radical_inverse(mut index: u64, base: u32) -> f64 {
    let inverse_base = 1.0 / f64::from(base);
    let mut factor = inverse_base;
    let mut result = 0.0;
    while index != 0 {
        let digit = index % u64::from(base);
        result += digit as f64 * factor;
        index /= u64::from(base);
        factor *= inverse_base;
    }
    result
}

/// Deterministic-sampling failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RngError {
    /// Requested dimensionality is zero or above the frozen prime table.
    UnsupportedDimensions(usize),
    /// Sequence index cannot be incremented without overflow.
    IndexOverflow,
}

impl RngError {
    /// Returns the stable machine-readable error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedDimensions(_) => "rng.unsupported_dimensions",
            Self::IndexOverflow => "rng.index_overflow",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn philox_matches_random123_zero_vector() {
        assert_eq!(
            philox4x32_10([0; 4], [0; 2]),
            [0x6627_e8d5, 0xe169_c58d, 0xbc57_ac4c, 0x9b00_dbd8]
        );
    }

    #[test]
    fn key_changes_are_independent_and_unit_mapping_is_half_open() {
        let base = RandomKey {
            seed: 7,
            population: StableRandomId::from_text("p"),
            lifecycle_event: StableRandomId::from_text("e"),
            particle: ParticleId(9),
            sampling_dimension: 2,
            draw_index: 0,
        };
        let first = CounterRng::sample_u64(base);
        let second = CounterRng::sample_u64(RandomKey {
            draw_index: 1,
            ..base
        });
        assert_ne!(first, second);
        let unit = CounterRng::sample_unit(base);
        assert!((0.0..1.0).contains(&unit));
    }

    #[test]
    fn stable_text_digest_and_halton_points_are_frozen() {
        assert_eq!(
            StableRandomId::from_text("population-a"),
            StableRandomId::from_text("population-a")
        );
        assert_ne!(
            StableRandomId::from_text("population-a"),
            StableRandomId::from_text("population-b")
        );
        assert_eq!(
            LowDiscrepancySampler::sample(0, 3),
            Ok(vec![0.5, 1.0 / 3.0, 0.2])
        );
    }
}
