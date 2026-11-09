//! Codes to packed words: the inverse of the decode's `Packing::unpack`, so a calibrated
//! producer's codes become its own bytes again.

/// Two four-bit codes, even column low. Shared by the rowwise W4A8 producers.
pub(super) fn nibble_pair(low: u8, high: u8) -> u8 {
    (low & 15) | ((high & 15) << 4)
}
