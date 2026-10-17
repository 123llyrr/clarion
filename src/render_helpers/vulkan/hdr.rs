//! HDR10 KMS metadata encoding and legacy transfer-function test helpers.
//!
//! The production color pipeline is defined in `crate::color`: each surface is
//! decoded into an extended floating-point scene, then encoded for its output.
//! This module keeps the Linux metadata ABI and older CPU PQ regression helpers.

/// HDR10/PQ reference white, in nits.
pub const HDR10_REFERENCE_WHITE_NITS: f32 = 203.0;
/// SMPTE ST 2084 nominal peak luminance, in nits.
pub const PQ_PEAK_NITS: f32 = 10_000.0;
/// DRM colorspace enum value for BT.2020 RGB.
pub const DRM_COLORSPACE_BT2020_RGB: u64 = 9;

const PQ_M1: f32 = 2610.0 / 16384.0;
const PQ_M2: f32 = 2523.0 / 32.0;
const PQ_C1: f32 = 3424.0 / 4096.0;
const PQ_C2: f32 = 2413.0 / 128.0;
const PQ_C3: f32 = 2392.0 / 128.0;

/// Build the Linux `struct hdr_output_metadata` HDR10 static metadata blob.
///
/// The byte layout is the UAPI layout: a u32 metadata type followed by the
/// CTA-861-G type-1 payload and the C struct's trailing alignment padding.
/// The kernel rejects blobs whose length differs from the native UAPI struct.
/// Values are encoded in the units required by the DRM property.
pub fn hdr10_metadata_blob() -> [u8; std::mem::size_of::<drm_ffi::hdr_output_metadata>()] {
    hdr10_metadata_blob_for(1000.0, 0.0001, 400.0)
}

/// Encode the actual display target's luminance volume, in DRM UAPI units.
/// Unknown EDID values must be resolved by the output policy before this call.
pub fn hdr10_metadata_blob_for(
    peak_luminance: f32,
    min_luminance: f32,
    max_fall: f32,
) -> [u8; std::mem::size_of::<drm_ffi::hdr_output_metadata>()] {
    let finite = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
    let peak = finite(peak_luminance, 1000.0).clamp(1.0, 10_000.0);
    let minimum = finite(min_luminance, 0.005).clamp(0.0, peak);
    let average = finite(max_fall, peak).clamp(0.0, peak);
    let mut metadata = [0u8; std::mem::size_of::<drm_ffi::hdr_output_metadata>()];
    metadata[4] = 2; // SMPTE ST 2084 (PQ) EOTF
    let primaries: [(f32, f32); 3] = [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)];
    for (index, (x, y)) in primaries.into_iter().enumerate() {
        let offset = 6 + index * 4;
        metadata[offset..offset + 2]
            .copy_from_slice(&((x * 50_000.0).round() as u16).to_ne_bytes());
        metadata[offset + 2..offset + 4]
            .copy_from_slice(&((y * 50_000.0).round() as u16).to_ne_bytes());
    }
    metadata[18..20].copy_from_slice(&((0.3127_f32 * 50_000.0).round() as u16).to_ne_bytes());
    metadata[20..22].copy_from_slice(&((0.3290_f32 * 50_000.0).round() as u16).to_ne_bytes());
    metadata[22..24].copy_from_slice(&(peak.round() as u16).to_ne_bytes());
    metadata[24..26].copy_from_slice(&((minimum * 10_000.0).round() as u16).to_ne_bytes());
    metadata[26..28].copy_from_slice(&(peak.round() as u16).to_ne_bytes());
    metadata[28..30].copy_from_slice(&(average.round() as u16).to_ne_bytes());
    metadata
}

/// Encode absolute luminance (cd/m²) using SMPTE ST 2084 (PQ).
pub fn pq_encode(luminance_nits: f32) -> f32 {
    let l = (luminance_nits / PQ_PEAK_NITS).clamp(0.0, 1.0).max(0.0);
    if l == 0.0 {
        return 0.0;
    }
    let lm = l.powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * lm) / (1.0 + PQ_C3 * lm)).powf(PQ_M2)
}

/// Decode a normalized SMPTE ST 2084 (PQ) value to absolute luminance.
pub fn pq_decode(encoded: f32) -> f32 {
    let e = encoded.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    let numerator = (e - PQ_C1).max(0.0);
    let denominator = PQ_C2 - PQ_C3 * e;
    if denominator <= 0.0 {
        return PQ_PEAK_NITS;
    }
    PQ_PEAK_NITS * (numerator / denominator).powf(1.0 / PQ_M1)
}

/// Convert an SDR relative-linear channel to HDR10 luminance.
///
/// SDR white is mapped to the HDR10 reference white (203 nits), not to the
/// PQ peak. Values outside the SDR range are retained up to the PQ peak.
pub fn sdr_linear_to_hdr10(value: f32) -> f32 {
    pq_encode(value.max(0.0) * HDR10_REFERENCE_WHITE_NITS)
}

/// Convert HDR10 luminance to an SDR relative-linear channel.
pub fn hdr10_to_sdr_linear(encoded: f32) -> f32 {
    pq_decode(encoded) / HDR10_REFERENCE_WHITE_NITS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pq_endpoints_are_stable() {
        assert_eq!(pq_encode(0.0), 0.0);
        assert_eq!(pq_decode(1.0), PQ_PEAK_NITS);
        assert_eq!(pq_encode(PQ_PEAK_NITS), 1.0);
    }

    #[test]
    fn pq_round_trip_is_monotonic() {
        let values = [0.0, 0.001, 0.1, 1.0, 10.0, 203.0, 1_000.0, 10_000.0];
        let mut previous = 0.0;
        for luminance in values {
            let encoded = pq_encode(luminance);
            assert!(encoded >= previous);
            assert!((pq_decode(encoded) - luminance).abs() <= luminance.max(1.0) * 2e-4);
            previous = encoded;
        }
    }

    #[test]
    fn sdr_white_maps_to_hdr_reference_white() {
        let encoded = sdr_linear_to_hdr10(1.0);
        assert!((pq_decode(encoded) - HDR10_REFERENCE_WHITE_NITS).abs() < 0.01);
        assert!((hdr10_to_sdr_linear(encoded) - 1.0).abs() < 2e-4);
    }

    #[test]
    fn display_luminance_metadata_preserves_edid_peak_and_black_level() {
        let metadata = hdr10_metadata_blob_for(616.884, 0.005, 400.0);
        let read = |offset| u16::from_ne_bytes([metadata[offset], metadata[offset + 1]]);
        assert_eq!(read(22), 617);
        assert_eq!(read(24), 50);
        assert_eq!(read(26), 617);
        assert_eq!(read(28), 400);
        let metadata = hdr10_metadata_blob_for(f32::NAN, f32::INFINITY, 20_000.0);
        assert_eq!(u16::from_ne_bytes([metadata[22], metadata[23]]), 1000);
        assert_eq!(u16::from_ne_bytes([metadata[28], metadata[29]]), 1000);
    }

    #[test]
    fn hdr10_metadata_uses_drm_units() {
        let metadata = hdr10_metadata_blob();
        assert_eq!(
            metadata.len(),
            std::mem::size_of::<drm_ffi::hdr_output_metadata>()
        );
        assert_eq!(metadata[0..4], [0; 4]);
        assert_eq!(metadata[4], 2);
        assert_eq!(u16::from_ne_bytes([metadata[22], metadata[23]]), 1000);
        assert_eq!(u16::from_ne_bytes([metadata[28], metadata[29]]), 400);
    }

    #[test]
    fn hdr10_metadata_matches_kernel_abi_including_padding() {
        let metadata = hdr10_metadata_blob();
        let payload = std::mem::offset_of!(drm_ffi::hdr_output_metadata, __bindgen_anon_1);
        let primaries =
            payload + std::mem::offset_of!(drm_ffi::hdr_metadata_infoframe, display_primaries);
        let max_luminance = payload
            + std::mem::offset_of!(
                drm_ffi::hdr_metadata_infoframe,
                max_display_mastering_luminance
            );
        let min_luminance = payload
            + std::mem::offset_of!(
                drm_ffi::hdr_metadata_infoframe,
                min_display_mastering_luminance
            );
        let max_cll = payload + std::mem::offset_of!(drm_ffi::hdr_metadata_infoframe, max_cll);
        let max_fall = payload + std::mem::offset_of!(drm_ffi::hdr_metadata_infoframe, max_fall);
        let read = |offset| u16::from_ne_bytes([metadata[offset], metadata[offset + 1]]);
        assert_eq!(metadata.len(), 32);
        assert_eq!(metadata[payload], 2);
        assert_eq!(metadata[payload + 1], 0);
        assert_eq!(read(primaries), 35_400);
        assert_eq!(read(max_luminance), 1000);
        assert_eq!(read(min_luminance), 1);
        assert_eq!(read(max_cll), 1000);
        assert_eq!(read(max_fall), 400);
        assert_eq!(&metadata[30..], &[0, 0]);
    }
}
