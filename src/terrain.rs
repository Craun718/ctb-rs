use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};

use crate::CtbError;

pub const HEIGHTMAP_TILE_SIZE: usize = 65;
pub const HEIGHTMAP_SAMPLE_COUNT: usize = HEIGHTMAP_TILE_SIZE * HEIGHTMAP_TILE_SIZE;
pub const WATER_MASK_SIZE: usize = 256 * 256;
const COMPACT_PAYLOAD_SIZE: usize = HEIGHTMAP_SAMPLE_COUNT * 2 + 2;
const DETAILED_PAYLOAD_SIZE: usize = HEIGHTMAP_SAMPLE_COUNT * 2 + 1 + WATER_MASK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildMask(u8);

impl ChildMask {
    pub const SOUTH_WEST: u8 = 0b0001;
    pub const SOUTH_EAST: u8 = 0b0010;
    pub const NORTH_WEST: u8 = 0b0100;
    pub const NORTH_EAST: u8 = 0b1000;

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, child: u8) -> bool {
        self.0 & child == child
    }

    pub fn set(&mut self, child: u8, present: bool) {
        if present {
            self.0 |= child;
        } else {
            self.0 &= !child;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaterMask {
    AllLand,
    AllWater,
    Detailed(Vec<u8>),
}

impl WaterMask {
    fn bytes(&self) -> Result<Vec<u8>, CtbError> {
        match self {
            Self::AllLand => Ok(vec![0]),
            Self::AllWater => Ok(vec![1]),
            Self::Detailed(mask) if mask.len() == WATER_MASK_SIZE => Ok(mask.clone()),
            Self::Detailed(mask) => Err(CtbError::InvalidWaterMaskLength(mask.len())),
        }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self, CtbError> {
        match bytes {
            [0] => Ok(Self::AllLand),
            [1] => Ok(Self::AllWater),
            mask if mask.len() == WATER_MASK_SIZE => Ok(Self::Detailed(mask.to_vec())),
            mask => Err(CtbError::InvalidWaterMaskLength(mask.len())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightmapTerrain {
    /// CTB heightmap values: 0.2 m units above the -1000 m datum.
    pub heights: Vec<u16>,
    pub children: ChildMask,
    pub water_mask: WaterMask,
}

impl HeightmapTerrain {
    pub fn from_sampled_meters(heights: &[f64], children: ChildMask) -> Result<Self, CtbError> {
        let mut quantized = Vec::with_capacity(heights.len());
        for height in heights {
            quantized.push(encode_ctb_height(*height)?);
        }
        Self::new(quantized, children, WaterMask::AllLand)
    }

    pub fn new(
        heights: Vec<u16>,
        children: ChildMask,
        water_mask: WaterMask,
    ) -> Result<Self, CtbError> {
        if heights.len() != HEIGHTMAP_SAMPLE_COUNT {
            return Err(CtbError::InvalidTerrainPayloadLength {
                expected: HEIGHTMAP_SAMPLE_COUNT,
                actual: heights.len(),
            });
        }
        water_mask.bytes()?;
        Ok(Self {
            heights,
            children,
            water_mask,
        })
    }

    pub fn encode_raw(&self) -> Result<Vec<u8>, CtbError> {
        let mask = self.water_mask.bytes()?;
        let mut encoded = Vec::with_capacity(HEIGHTMAP_SAMPLE_COUNT * 2 + 1 + mask.len());
        for height in &self.heights {
            encoded.extend_from_slice(&height.to_le_bytes());
        }
        encoded.push(self.children.bits());
        encoded.extend_from_slice(&mask);
        Ok(encoded)
    }

    pub fn decode_raw(encoded: &[u8]) -> Result<Self, CtbError> {
        if encoded.len() != COMPACT_PAYLOAD_SIZE && encoded.len() != DETAILED_PAYLOAD_SIZE {
            return Err(CtbError::InvalidTerrainPayloadLength {
                expected: COMPACT_PAYLOAD_SIZE,
                actual: encoded.len(),
            });
        }

        let mut heights = Vec::with_capacity(HEIGHTMAP_SAMPLE_COUNT);
        for bytes in encoded[..HEIGHTMAP_SAMPLE_COUNT * 2].chunks_exact(2) {
            heights.push(u16::from_le_bytes([bytes[0], bytes[1]]));
        }
        let children = ChildMask(encoded[HEIGHTMAP_SAMPLE_COUNT * 2]);
        let water_mask = WaterMask::from_bytes(&encoded[HEIGHTMAP_SAMPLE_COUNT * 2 + 1..])?;
        Self::new(heights, children, water_mask)
    }

    pub fn encode_gzip(&self) -> Result<Vec<u8>, CtbError> {
        let raw = self.encode_raw()?;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&raw)
            .map_err(|error| CtbError::TerrainCompression(error.to_string()))?;
        encoder
            .finish()
            .map_err(|error| CtbError::TerrainCompression(error.to_string()))
    }

    pub fn decode_gzip(encoded: &[u8]) -> Result<Self, CtbError> {
        let mut decoder = GzDecoder::new(encoded);
        let mut raw = Vec::with_capacity(DETAILED_PAYLOAD_SIZE);
        decoder
            .by_ref()
            .take((DETAILED_PAYLOAD_SIZE + 1) as u64)
            .read_to_end(&mut raw)
            .map_err(|error| CtbError::TerrainCompression(error.to_string()))?;
        if raw.len() > DETAILED_PAYLOAD_SIZE {
            return Err(CtbError::InvalidTerrainPayloadLength {
                expected: COMPACT_PAYLOAD_SIZE,
                actual: raw.len(),
            });
        }
        Self::decode_raw(&raw)
    }

    pub fn write_gzip(&self, path: impl AsRef<Path>) -> Result<(), CtbError> {
        fs::write(path, self.encode_gzip()?)
            .map_err(|error| CtbError::TerrainCompression(error.to_string()))
    }

    pub fn read_gzip(path: impl AsRef<Path>) -> Result<Self, CtbError> {
        let encoded =
            fs::read(path).map_err(|error| CtbError::TerrainCompression(error.to_string()))?;
        Self::decode_gzip(&encoded)
    }
}

fn encode_ctb_height(height: f64) -> Result<u16, CtbError> {
    if !height.is_finite() {
        return Err(CtbError::InvalidElevation(height));
    }
    let height = height as f32;
    if !height.is_finite() {
        return Err(CtbError::InvalidElevation(f64::from(height)));
    }
    let encoded = (height + 1000.0) * 5.0;
    if !encoded.is_finite() || encoded < 0.0 || encoded > f32::from(u16::MAX) {
        return Err(CtbError::InvalidElevation(f64::from(height)));
    }
    Ok(encoded.trunc() as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_all_land_payload_round_trips() -> Result<(), CtbError> {
        let mut children = ChildMask::empty();
        children.set(ChildMask::SOUTH_WEST, true);
        children.set(ChildMask::NORTH_EAST, true);
        let terrain = HeightmapTerrain::new(
            vec![12; HEIGHTMAP_SAMPLE_COUNT],
            children,
            WaterMask::AllLand,
        )?;
        let encoded = terrain.encode_raw()?;
        assert_eq!(encoded.len(), HEIGHTMAP_SAMPLE_COUNT * 2 + 2);
        assert_eq!(HeightmapTerrain::decode_raw(&encoded)?, terrain);
        Ok(())
    }

    #[test]
    fn detailed_water_mask_round_trips() -> Result<(), CtbError> {
        let terrain = HeightmapTerrain::new(
            vec![0; HEIGHTMAP_SAMPLE_COUNT],
            ChildMask::empty(),
            WaterMask::Detailed(vec![127; WATER_MASK_SIZE]),
        )?;
        assert_eq!(
            HeightmapTerrain::decode_raw(&terrain.encode_raw()?)?,
            terrain
        );
        Ok(())
    }

    #[test]
    fn sampled_meters_use_ctb_offset_scale_and_truncation() -> Result<(), CtbError> {
        let mut heights = vec![0.0; HEIGHTMAP_SAMPLE_COUNT];
        heights[0] = -999.5;
        heights[1] = 1.5;
        let terrain = HeightmapTerrain::from_sampled_meters(&heights, ChildMask::empty())?;
        assert_eq!(terrain.heights[0], 2);
        assert_eq!(terrain.heights[1], 5007);
        assert_eq!(terrain.water_mask, WaterMask::AllLand);
        Ok(())
    }

    #[test]
    fn sampled_meters_reject_invalid_elevations() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 12107.2, -1000.1] {
            let mut heights = vec![0.0; HEIGHTMAP_SAMPLE_COUNT];
            heights[0] = value;
            assert!(matches!(
                HeightmapTerrain::from_sampled_meters(&heights, ChildMask::empty()),
                Err(CtbError::InvalidElevation(_))
            ));
        }
    }

    #[test]
    fn sampled_meters_accept_ctb_boundaries_and_require_heightmap_size() -> Result<(), CtbError> {
        let mut heights = vec![0.0; HEIGHTMAP_SAMPLE_COUNT];
        heights[0] = -1000.0;
        heights[1] = 12107.0;
        let terrain = HeightmapTerrain::from_sampled_meters(&heights, ChildMask::empty())?;
        assert_eq!(terrain.heights[0], 0);
        assert_eq!(terrain.heights[1], u16::MAX);
        assert!(matches!(
            HeightmapTerrain::from_sampled_meters(&[], ChildMask::empty()),
            Err(CtbError::InvalidTerrainPayloadLength { .. })
        ));
        Ok(())
    }

    #[test]
    fn gzip_round_trips_compact_and_detailed_heightmaps() -> Result<(), CtbError> {
        let compact = HeightmapTerrain::new(
            vec![4; HEIGHTMAP_SAMPLE_COUNT],
            ChildMask::empty(),
            WaterMask::AllLand,
        )?;
        let detailed = HeightmapTerrain::new(
            vec![4; HEIGHTMAP_SAMPLE_COUNT],
            ChildMask::empty(),
            WaterMask::Detailed(vec![123; WATER_MASK_SIZE]),
        )?;
        assert_eq!(
            HeightmapTerrain::decode_gzip(&compact.encode_gzip()?)?,
            compact
        );
        assert_eq!(
            HeightmapTerrain::decode_gzip(&detailed.encode_gzip()?)?,
            detailed
        );
        Ok(())
    }

    #[test]
    fn gzip_rejects_invalid_and_oversized_payloads() -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            HeightmapTerrain::decode_gzip(b"not gzip"),
            Err(CtbError::TerrainCompression(_))
        ));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&vec![0; DETAILED_PAYLOAD_SIZE + 1])?;
        let oversized = encoder.finish()?;
        assert!(matches!(
            HeightmapTerrain::decode_gzip(&oversized),
            Err(CtbError::InvalidTerrainPayloadLength { .. })
        ));
        Ok(())
    }

    #[test]
    fn gzip_file_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let terrain = HeightmapTerrain::new(
            vec![9; HEIGHTMAP_SAMPLE_COUNT],
            ChildMask::empty(),
            WaterMask::AllLand,
        )?;
        let path =
            std::env::temp_dir().join(format!("ctb-rs-heightmap-{}.terrain", std::process::id()));
        terrain.write_gzip(&path)?;
        assert_eq!(HeightmapTerrain::read_gzip(&path)?, terrain);
        fs::remove_file(path)?;
        Ok(())
    }
}
