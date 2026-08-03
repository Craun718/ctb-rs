use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq)]
pub enum CtbError {
    InvalidBounds,
    InvalidTileSize(u32),
    InvalidZoom(u8),
    InvalidZoomRange { start: u8, end: u8, maximum: u8 },
    CoordinateOutsideGrid { x: f64, y: f64 },
    InvalidRasterDimensions { width: u32, height: u32 },
    InvalidRasterWindow,
    RasterRead(String),
    UnsupportedRaster(String),
    RasterOutsideGrid,
    UnsupportedCrs(String),
    MissingCrs,
    NoDataEncountered,
    InvalidElevation(f64),
    TerrainCompression(String),
    TilesetIo(String),
    InvalidTerrainPayloadLength { expected: usize, actual: usize },
    InvalidWaterMaskLength(usize),
}

impl Display for CtbError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBounds => {
                write!(formatter, "bounds must have finite, increasing coordinates")
            }
            Self::InvalidTileSize(size) => {
                write!(formatter, "tile size must be at least two, got {size}")
            }
            Self::InvalidZoom(zoom) => write!(
                formatter,
                "zoom level {zoom} exceeds the supported grid range"
            ),
            Self::InvalidZoomRange {
                start,
                end,
                maximum,
            } => write!(
                formatter,
                "invalid zoom range {start}..={end}; require {maximum} >= start >= end"
            ),
            Self::CoordinateOutsideGrid { x, y } => {
                write!(
                    formatter,
                    "coordinate ({x}, {y}) lies outside the grid extent"
                )
            }
            Self::InvalidRasterDimensions { width, height } => {
                write!(
                    formatter,
                    "raster dimensions must be non-zero, got {width}x{height}"
                )
            }
            Self::InvalidRasterWindow => {
                write!(
                    formatter,
                    "raster window lies outside the available raster data"
                )
            }
            Self::RasterRead(message) => write!(formatter, "could not read raster: {message}"),
            Self::UnsupportedRaster(message) => write!(formatter, "unsupported raster: {message}"),
            Self::RasterOutsideGrid => {
                write!(formatter, "raster does not intersect the terrain grid")
            }
            Self::UnsupportedCrs(value) => write!(formatter, "unsupported CRS: {value}"),
            Self::MissingCrs => write!(formatter, "source raster has no CRS"),
            Self::NoDataEncountered => write!(formatter, "source raster contains a NoData sample"),
            Self::InvalidElevation(value) => {
                write!(
                    formatter,
                    "elevation {value} cannot be encoded as a CTB u16 terrain height"
                )
            }
            Self::TerrainCompression(message) => write!(formatter, "terrain gzip error: {message}"),
            Self::TilesetIo(message) => write!(formatter, "tileset I/O error: {message}"),
            Self::InvalidTerrainPayloadLength { expected, actual } => {
                write!(
                    formatter,
                    "invalid terrain payload length: compact payloads are {expected} bytes; got {actual} bytes"
                )
            }
            Self::InvalidWaterMaskLength(length) => {
                write!(
                    formatter,
                    "water mask must contain one or 65536 bytes, got {length}"
                )
            }
        }
    }
}

impl std::error::Error for CtbError {}
