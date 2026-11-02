use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use geotiff_reader::GeoTiffFile;
use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

use crate::{
    CtbError,
    raster::{RasterSampleType, WindowRequest},
};

const MAX_VRT_DEPTH: usize = 16;

pub struct VrtReader {
    width: u32,
    height: u32,
    srs: String,
    geo_transform: [f64; 6],
    band: VrtBand,
    sources: Vec<OpenedSource>,
}

#[derive(Debug)]
struct VrtBand {
    sample_type: RasterSampleType,
    no_data: Option<f64>,
}

struct OpenedSource {
    definition: SourceDefinition,
    raster: SourceRaster,
}

enum SourceRaster {
    GeoTiff(Arc<GeoTiffFile>),
    Vrt(Arc<VrtReader>),
}

#[derive(Debug, Clone)]
struct SourceDefinition {
    path: PathBuf,
    relative_to_vrt: bool,
    band: usize,
    source_rect: PixelRect,
    destination_rect: PixelRect,
    no_data: Option<f64>,
    scale_ratio: f64,
    scale_offset: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelRect {
    fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let x_end = (self.x + self.width).min(other.x + other.width);
        let y_end = (self.y + self.height).min(other.y + other.height);
        if x_end <= x || y_end <= y {
            return None;
        }
        Some(Self {
            x,
            y,
            width: x_end - x,
            height: y_end - y,
        })
    }
}

impl VrtReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CtbError> {
        let path = path.as_ref();
        let document = VrtDocument::parse_file(path)?;
        let mut visited = vec![absolute_path(path)?];
        Self::from_document(&document, path, &mut visited, 0)
    }

    fn from_document(
        document: &VrtDocument,
        path: &Path,
        visited: &mut Vec<PathBuf>,
        depth: usize,
    ) -> Result<Self, CtbError> {
        if depth > MAX_VRT_DEPTH {
            return Err(CtbError::UnsupportedRaster(
                "VRT source nesting exceeds the supported depth".to_owned(),
            ));
        }
        if document
            .subclass
            .as_deref()
            .is_some_and(|value| value != "VRTDataset")
        {
            return Err(CtbError::UnsupportedRaster(format!(
                "VRT subclass {:?} is not supported by the standard VRT reader",
                document.subclass
            )));
        }
        if document.bands.len() != 1 {
            return Err(CtbError::UnsupportedRaster(format!(
                "expected one VRT elevation band, found {} bands",
                document.bands.len()
            )));
        }
        let band = &document.bands[0];
        if band.pixel_function.is_some() {
            return Err(CtbError::UnsupportedRaster(
                "VRT pixel functions are not supported".to_owned(),
            ));
        }

        let mut sources = Vec::with_capacity(band.sources.len());
        for definition in &band.sources {
            let source_path = definition.resolve_path(path)?;
            let absolute = absolute_path(&source_path)?;
            if visited.iter().any(|visited| visited == &absolute) {
                return Err(CtbError::UnsupportedRaster(
                    "VRT source recursion detected".to_owned(),
                ));
            }
            visited.push(absolute);
            let raster = match crate::geotiff::detect_raster_format(&source_path)? {
                crate::geotiff::RasterFormat::GeoTiff => {
                    SourceRaster::GeoTiff(Arc::new(open_geotiff(&source_path)?))
                }
                crate::geotiff::RasterFormat::Vrt => {
                    let nested_document = VrtDocument::parse_file(&source_path)?;
                    SourceRaster::Vrt(Arc::new(Self::from_document(
                        &nested_document,
                        &source_path,
                        visited,
                        depth + 1,
                    )?))
                }
            };
            visited.pop();
            sources.push(OpenedSource {
                definition: definition.clone(),
                raster,
            });
        }

        Ok(Self {
            width: document.width,
            height: document.height,
            srs: document.srs.clone(),
            geo_transform: document.geo_transform,
            band: VrtBand {
                sample_type: band.sample_type,
                no_data: band.no_data,
            },
            sources,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn band_count(&self) -> usize {
        1
    }

    pub fn srs(&self) -> &str {
        &self.srs
    }

    pub fn geo_transform(&self) -> &[f64; 6] {
        &self.geo_transform
    }

    pub fn band_sample_type(&self) -> RasterSampleType {
        self.band.sample_type
    }

    pub fn band_no_data(&self) -> Option<f64> {
        self.band.no_data
    }

    pub fn read_window(&self, request: WindowRequest) -> Result<Vec<f64>, CtbError> {
        let requested = PixelRect {
            x: request.x,
            y: request.y,
            width: request.width,
            height: request.height,
        };
        let width = usize::try_from(request.width).map_err(|_| CtbError::InvalidRasterWindow)?;
        let height = usize::try_from(request.height).map_err(|_| CtbError::InvalidRasterWindow)?;
        let count = width
            .checked_mul(height)
            .ok_or(CtbError::InvalidRasterWindow)?;
        let mut output = vec![0.0_f64; count];
        let mut contributed = false;
        for source in &self.sources {
            let Some(destination) = source.definition.destination_rect.intersection(requested)
            else {
                continue;
            };
            contributed = true;
            source.copy_into(destination, requested.width, &mut output)?;
        }
        if !contributed {
            return Err(CtbError::RasterRead(
                "no VRT sources contribute to the requested window".to_owned(),
            ));
        }
        Ok(output)
    }
}

impl OpenedSource {
    fn copy_into(
        &self,
        destination: PixelRect,
        destination_width: u32,
        output: &mut [f64],
    ) -> Result<(), CtbError> {
        let definition = &self.definition;
        let source_request = WindowRequest {
            x: definition.source_rect.x,
            y: definition.source_rect.y,
            width: definition.source_rect.width,
            height: definition.source_rect.height,
            overview: 0,
        };
        let samples = match &self.raster {
            SourceRaster::GeoTiff(file) => {
                crate::geotiff::read_geotiff_band_window(file, definition.band, source_request)?
            }
            SourceRaster::Vrt(reader) => reader.read_window(source_request)?,
        };

        for row in 0..destination.height {
            for column in 0..destination.width {
                let destination_x = destination.x + column;
                let destination_y = destination.y + row;
                let relative_x = f64::from(destination_x - definition.destination_rect.x)
                    .mul_add(
                        f64::from(definition.source_rect.width)
                            / f64::from(definition.destination_rect.width),
                        f64::from(definition.source_rect.x),
                    )
                    .floor();
                let relative_y = f64::from(destination_y - definition.destination_rect.y)
                    .mul_add(
                        f64::from(definition.source_rect.height)
                            / f64::from(definition.destination_rect.height),
                        f64::from(definition.source_rect.y),
                    )
                    .floor();
                let source_x = relative_x as i64;
                let source_y = relative_y as i64;
                if source_x < i64::from(definition.source_rect.x)
                    || source_y < i64::from(definition.source_rect.y)
                    || source_x
                        >= i64::from(definition.source_rect.x + definition.source_rect.width)
                    || source_y
                        >= i64::from(definition.source_rect.y + definition.source_rect.height)
                {
                    continue;
                }
                let source_index = usize::try_from(
                    (source_y - i64::from(definition.source_rect.y))
                        .checked_mul(i64::from(definition.source_rect.width))
                        .and_then(|value| {
                            value.checked_add(source_x - i64::from(definition.source_rect.x))
                        })
                        .ok_or(CtbError::RasterRead(
                            "VRT source sample offset overflow".to_owned(),
                        ))?,
                )
                .map_err(|_| {
                    CtbError::RasterRead("VRT source sample offset overflow".to_owned())
                })?;
                let Some(sample) = samples.get(source_index) else {
                    return Err(CtbError::RasterRead(
                        "VRT source window is smaller than its declared rectangle".to_owned(),
                    ));
                };
                if definition.no_data.is_some_and(|no_data| no_data == *sample) {
                    continue;
                }
                let value = sample.mul_add(definition.scale_ratio, definition.scale_offset);
                let output_index = usize::try_from(
                    u64::from(destination_y)
                        .checked_mul(u64::from(destination_width))
                        .and_then(|value| value.checked_add(u64::from(destination_x)))
                        .ok_or(CtbError::RasterRead(
                            "VRT destination sample offset overflow".to_owned(),
                        ))?,
                )
                .map_err(|_| {
                    CtbError::RasterRead("VRT destination sample offset overflow".to_owned())
                })?;
                let Some(target) = output.get_mut(output_index) else {
                    return Err(CtbError::RasterRead(
                        "VRT destination window is smaller than its requested rectangle".to_owned(),
                    ));
                };
                *target = value;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct VrtDocument {
    width: u32,
    height: u32,
    subclass: Option<String>,
    srs: String,
    geo_transform: [f64; 6],
    bands: Vec<VrtBandDefinition>,
}

#[derive(Debug)]
struct VrtBandDefinition {
    sample_type: RasterSampleType,
    no_data: Option<f64>,
    pixel_function: Option<String>,
    sources: Vec<SourceDefinition>,
}

impl SourceDefinition {
    fn resolve_path(&self, vrt_path: &Path) -> Result<PathBuf, CtbError> {
        if !self.relative_to_vrt {
            return Ok(self.path.clone());
        }
        let parent = vrt_path.parent().ok_or_else(|| {
            CtbError::RasterRead(format!(
                "relative VRT source {:?} has no parent VRT directory",
                self.path
            ))
        })?;
        Ok(parent.join(&self.path))
    }
}

impl VrtDocument {
    fn parse_file(path: &Path) -> Result<Self, CtbError> {
        let mut file = fs::File::open(path)
            .map_err(|error| CtbError::RasterRead(format!("cannot open VRT {path:?}: {error}")))?;
        let mut xml = String::new();
        file.read_to_string(&mut xml)
            .map_err(|error| CtbError::RasterRead(format!("cannot read VRT {path:?}: {error}")))?;
        Self::parse(&xml)
    }

    fn parse(xml: &str) -> Result<Self, CtbError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buffer = Vec::new();
        let mut document = None;
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Start(event)) if event.name().as_ref() == b"VRTDataset" => {
                    document = Some(parse_dataset(&mut reader, &event)?);
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => {
                    return Err(CtbError::RasterRead(format!(
                        "VRT XML parsing error at byte {}: {error}",
                        reader.buffer_position()
                    )));
                }
            }
            buffer.clear();
        }
        document.ok_or_else(|| CtbError::RasterRead("VRT XML has no VRTDataset element".to_owned()))
    }
}

fn parse_dataset(reader: &mut Reader<&[u8]>, start: &BytesStart) -> Result<VrtDocument, CtbError> {
    let width = parse_u32_attribute(start, b"rasterXSize")?
        .ok_or_else(|| CtbError::RasterRead("VRTDataset has no rasterXSize".to_owned()))?;
    let height = parse_u32_attribute(start, b"rasterYSize")?
        .ok_or_else(|| CtbError::RasterRead("VRTDataset has no rasterYSize".to_owned()))?;
    let subclass = parse_string_attribute(start, b"subClass")?;
    let mut srs = None;
    let mut geo_transform = None;
    let mut bands = Vec::new();
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => match event.name().as_ref() {
                b"SRS" => srs = Some(parse_text(reader, b"SRS")?),
                b"GeoTransform" => {
                    let text = parse_text(reader, b"GeoTransform")?;
                    geo_transform = Some(parse_geo_transform(&text)?);
                }
                b"VRTRasterBand" => bands.push(parse_band(reader, &event)?),
                b"GDALWarpOptions" => {
                    return Err(CtbError::UnsupportedRaster(
                        "warped VRT datasets are not supported".to_owned(),
                    ));
                }
                _ => skip_element(reader, event.name().as_ref().to_vec())?,
            },
            Ok(Event::End(event)) if event.name().as_ref() == b"VRTDataset" => break,
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead("unexpected end of VRT XML".to_owned()));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT XML parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }

    Ok(VrtDocument {
        width,
        height,
        subclass,
        srs: srs.ok_or_else(|| CtbError::RasterRead("VRT has no SRS".to_owned()))?,
        geo_transform: geo_transform
            .ok_or_else(|| CtbError::RasterRead("VRT has no GeoTransform".to_owned()))?,
        bands,
    })
}

fn parse_band(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart,
) -> Result<VrtBandDefinition, CtbError> {
    let type_name = parse_string_attribute(start, b"dataType")?
        .ok_or_else(|| CtbError::RasterRead("VRT band has no dataType".to_owned()))?;
    let sample_type = parse_sample_type(&type_name)?;
    let mut no_data = None;
    let mut pixel_function = None;
    let mut sources = Vec::new();
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => match event.name().as_ref() {
                b"NoDataValue" => {
                    no_data = parse_f64_text(reader, b"NoDataValue")?;
                }
                b"PixelFunctionType" => {
                    pixel_function = Some(parse_text(reader, b"PixelFunctionType")?);
                }
                b"SimpleSource" | b"ComplexSource" => {
                    sources.push(parse_source(reader, &event)?);
                }
                _ => skip_element(reader, event.name().as_ref().to_vec())?,
            },
            Ok(Event::End(event)) if event.name().as_ref() == b"VRTRasterBand" => break,
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead(
                    "unexpected end of VRT band".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT band XML parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }

    Ok(VrtBandDefinition {
        sample_type,
        no_data,
        pixel_function,
        sources,
    })
}

fn parse_source(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart,
) -> Result<SourceDefinition, CtbError> {
    let complex = event_is_complex_source(start);
    let mut path = None;
    let mut relative_to_vrt = false;
    let mut band = None;
    let mut source_rect = None;
    let mut destination_rect = None;
    let mut no_data = None;
    let mut scale_ratio = 1.0;
    let mut scale_offset = 0.0;
    let end_name = if complex {
        b"ComplexSource".to_vec()
    } else {
        b"SimpleSource".to_vec()
    };
    let mut buffer = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => match event.name().as_ref() {
                b"SourceFilename" => {
                    relative_to_vrt = parse_string_attribute(&event, b"relativeToVRT")?
                        .is_some_and(|value| value == "1");
                    path = Some(PathBuf::from(parse_text(reader, b"SourceFilename")?));
                }
                b"SourceBand" => {
                    let text = parse_text(reader, b"SourceBand")?;
                    band = Some(text.parse::<usize>().map_err(|_| {
                        CtbError::RasterRead(format!("invalid VRT SourceBand {text:?}"))
                    })?);
                }
                b"SrcRect" => source_rect = Some(parse_rect(reader, &event, b"SrcRect")?),
                b"DstRect" => {
                    destination_rect = Some(parse_rect(reader, &event, b"DstRect")?);
                }
                b"NODATA" if complex => no_data = parse_f64_text(reader, b"NODATA")?,
                b"ScaleRatio" if complex => {
                    scale_ratio = parse_f64_text(reader, b"ScaleRatio")?.ok_or_else(|| {
                        CtbError::RasterRead("VRT ScaleRatio is empty".to_owned())
                    })?;
                }
                b"ScaleOffset" if complex => {
                    scale_offset = parse_f64_text(reader, b"ScaleOffset")?.ok_or_else(|| {
                        CtbError::RasterRead("VRT ScaleOffset is empty".to_owned())
                    })?;
                }
                _ => skip_element(reader, event.name().as_ref().to_vec())?,
            },
            Ok(Event::End(event)) if event.name().as_ref() == end_name.as_slice() => break,
            Ok(Event::Empty(event)) => match event.name().as_ref() {
                b"SrcRect" => source_rect = Some(parse_rect_start(&event)?),
                b"DstRect" => destination_rect = Some(parse_rect_start(&event)?),
                _ => {}
            },
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead(
                    "unexpected end of VRT source".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT source XML parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }

    let source_rect =
        source_rect.ok_or_else(|| CtbError::RasterRead("VRT source has no SrcRect".to_owned()))?;
    let destination_rect = destination_rect
        .ok_or_else(|| CtbError::RasterRead("VRT source has no DstRect".to_owned()))?;
    if source_rect.width == 0
        || source_rect.height == 0
        || destination_rect.width == 0
        || destination_rect.height == 0
    {
        return Err(CtbError::RasterRead(
            "VRT source rectangles must have positive dimensions".to_owned(),
        ));
    }
    Ok(SourceDefinition {
        path: path.ok_or_else(|| CtbError::RasterRead("VRT source has no filename".to_owned()))?,
        relative_to_vrt,
        band: band.ok_or_else(|| CtbError::RasterRead("VRT source has no band".to_owned()))?,
        source_rect,
        destination_rect,
        no_data,
        scale_ratio,
        scale_offset,
    })
}

fn parse_rect(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart,
    end_name: &[u8],
) -> Result<PixelRect, CtbError> {
    let rect = parse_rect_start(start)?;
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::End(event)) if event.name().as_ref() == end_name => return Ok(rect),
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead(
                    "unexpected end of VRT rectangle".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT rectangle XML parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }
}

fn parse_rect_start(start: &BytesStart) -> Result<PixelRect, CtbError> {
    let x = parse_u32_attribute(start, b"xOff")?
        .ok_or_else(|| CtbError::RasterRead("VRT rectangle has no xOff".to_owned()))?;
    let y = parse_u32_attribute(start, b"yOff")?
        .ok_or_else(|| CtbError::RasterRead("VRT rectangle has no yOff".to_owned()))?;
    let width = parse_u32_attribute(start, b"xSize")?
        .ok_or_else(|| CtbError::RasterRead("VRT rectangle has no xSize".to_owned()))?;
    let height = parse_u32_attribute(start, b"ySize")?
        .ok_or_else(|| CtbError::RasterRead("VRT rectangle has no ySize".to_owned()))?;
    Ok(PixelRect {
        x,
        y,
        width,
        height,
    })
}

fn parse_text(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> Result<String, CtbError> {
    let mut text = String::new();
    let mut buffer = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Text(event)) => {
                let decoded = event
                    .decode()
                    .map_err(|error| CtbError::RasterRead(format!("invalid VRT text: {error}")))?;
                let decoded = quick_xml::escape::unescape(&decoded).map_err(|error| {
                    CtbError::RasterRead(format!("invalid VRT text escape: {error}"))
                })?;
                text.push_str(&decoded);
            }
            Ok(Event::End(event)) if event.name().as_ref() == end_name => {
                return Ok(text.trim().to_owned());
            }
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead(
                    "unexpected end of VRT text".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT text parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }
}

fn parse_f64_text(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> Result<Option<f64>, CtbError> {
    let text = parse_text(reader, end_name)?;
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .map(Some)
        .map_err(|_| CtbError::RasterRead(format!("invalid VRT numeric value {text:?}")))
}

fn skip_element(reader: &mut Reader<&[u8]>, name: Vec<u8>) -> Result<(), CtbError> {
    let mut buffer = Vec::new();
    let mut depth = 1_usize;
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) if event.name().as_ref() == name.as_slice() => {
                depth = depth
                    .checked_add(1)
                    .ok_or_else(|| CtbError::RasterRead("VRT XML depth overflow".to_owned()))?;
            }
            Ok(Event::End(event)) if event.name().as_ref() == name.as_slice() => {
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            Ok(Event::Eof) => {
                return Err(CtbError::RasterRead("unexpected end of VRT XML".to_owned()));
            }
            Ok(_) => {}
            Err(error) => {
                return Err(CtbError::RasterRead(format!(
                    "VRT XML parsing error: {error}"
                )));
            }
        }
        buffer.clear();
    }
}

fn parse_string_attribute(event: &BytesStart, name: &[u8]) -> Result<Option<String>, CtbError> {
    for attribute in event.attributes() {
        let attribute = attribute
            .map_err(|error| CtbError::RasterRead(format!("invalid VRT XML attribute: {error}")))?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                String::from_utf8_lossy(attribute.value.as_ref()).into_owned(),
            ));
        }
    }
    Ok(None)
}

fn parse_u32_attribute(event: &BytesStart, name: &[u8]) -> Result<Option<u32>, CtbError> {
    parse_string_attribute(event, name)?
        .map(|value| {
            value.parse::<u32>().map_err(|_| {
                CtbError::RasterRead(format!("invalid VRT integer attribute {value:?}"))
            })
        })
        .transpose()
}

fn parse_geo_transform(text: &str) -> Result<[f64; 6], CtbError> {
    let parsed = text
        .split(',')
        .map(|value| {
            value.trim().parse::<f64>().map_err(|_| {
                CtbError::RasterRead(format!("invalid VRT GeoTransform value {value:?}"))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parsed.len() != 6 {
        return Err(CtbError::RasterRead(
            "VRT GeoTransform must contain six values".to_owned(),
        ));
    }
    let values = [
        parsed[0], parsed[1], parsed[2], parsed[3], parsed[4], parsed[5],
    ];
    Ok(values)
}

fn parse_sample_type(value: &str) -> Result<RasterSampleType, CtbError> {
    match value {
        "Byte" => Ok(RasterSampleType::Unsigned8),
        "Int8" => Ok(RasterSampleType::Signed8),
        "UInt16" => Ok(RasterSampleType::Unsigned16),
        "Int16" => Ok(RasterSampleType::Signed16),
        "UInt32" => Ok(RasterSampleType::Unsigned32),
        "Int32" => Ok(RasterSampleType::Signed32),
        "Float32" => Ok(RasterSampleType::Float32),
        "Float64" => Ok(RasterSampleType::Float64),
        _ => Err(CtbError::UnsupportedRaster(format!(
            "unsupported VRT dataType {value:?}"
        ))),
    }
}

fn event_is_complex_source(event: &BytesStart) -> bool {
    event.name().as_ref() == b"ComplexSource"
}

fn absolute_path(path: &Path) -> Result<PathBuf, CtbError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| CtbError::RasterRead(format!("cannot resolve VRT path {path:?}: {error}")))
}

pub fn resolve_vrt_epsg(srs: &str) -> Result<u16, CtbError> {
    let trimmed = srs.trim();
    if let Some(code) = trimmed.strip_prefix("EPSG:") {
        return code.trim().parse::<u16>().map_err(|_| {
            CtbError::UnsupportedCrs(format!("invalid VRT SRS authority code {code:?}"))
        });
    }
    let normalized = trimmed.replace([' ', '\n', '\r'], "");
    let patterns = [("EPSG\",", '"'), ("EPSG[", ']'), ("EPSG=", ';')];
    for (prefix, terminator) in patterns {
        if let Some(start) = normalized.find(prefix) {
            let digits_start = start + prefix.len();
            if normalized[digits_start..].starts_with('"') {
                // Handled by the escaped EPSG", pattern above when quotes were removed.
            }
            let digits = normalized[digits_start..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect::<String>();
            if !digits.is_empty() {
                return digits.parse::<u16>().map_err(|_| {
                    CtbError::UnsupportedCrs(format!(
                        "VRT SRS EPSG code {digits} is outside the supported range"
                    ))
                });
            }
            let _ = terminator;
        }
    }
    Err(CtbError::UnsupportedCrs(
        "VRT SRS does not expose an EPSG code usable by proj4rs".to_owned(),
    ))
}

fn open_geotiff(path: &Path) -> Result<GeoTiffFile, CtbError> {
    crate::geotiff::open_geotiff(path)
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf};

    use geotiff_writer::GeoTiffBuilder;
    use ndarray::Array2;

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = env::temp_dir().join(format!(
            "ctb-rs-vrt-{name}-{}-{}",
            std::process::id(),
            line!()
        ));
        fs::create_dir(&path).map_err(|error| {
            CtbError::RasterRead(format!("cannot create VRT test directory: {error}"))
        })?;
        Ok(path)
    }

    fn write_source(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let samples = Array2::from_shape_fn((4, 4), |(row, column)| (row * 4 + column) as f64);
        GeoTiffBuilder::new(4, 4)
            .geographic_epsg(4326)
            .pixel_scale(1.0, 1.0)
            .origin(-1.0, 1.0)
            .write_2d(path, samples.view())?;
        Ok(())
    }

    fn vrt_xml(width: u32, height: u32, band_body: &str) -> String {
        format!(
            r#"<VRTDataset rasterXSize="{width}" rasterYSize="{height}">
  <SRS>EPSG:4326</SRS>
  <GeoTransform>-1, 1, 0, 1, 0, -1</GeoTransform>
  <VRTRasterBand dataType="Float64" band="1">{band_body}</VRTRasterBand>
</VRTDataset>"#
        )
    }

    fn complex_source(
        filename: &str,
        src: (u32, u32, u32, u32),
        dst: (u32, u32, u32, u32),
    ) -> String {
        format!(
            r#"<ComplexSource>
  <SourceFilename relativeToVRT="1">{filename}</SourceFilename>
  <SourceBand>1</SourceBand>
  <SrcRect xOff="{}" yOff="{}" xSize="{}" ySize="{}" />
  <DstRect xOff="{}" yOff="{}" xSize="{}" ySize="{}" />
  <NODATA>5</NODATA>
  <ScaleRatio>2</ScaleRatio>
  <ScaleOffset>1</ScaleOffset>
</ComplexSource>"#,
            src.0, src.1, src.2, src.3, dst.0, dst.1, dst.2, dst.3
        )
    }

    #[test]
    fn resolves_epsg_from_authority_forms() -> Result<(), CtbError> {
        assert_eq!(resolve_vrt_epsg("EPSG:4326")?, 4326);
        assert_eq!(resolve_vrt_epsg("ID[\"EPSG\",3857]")?, 3857);
        Ok(())
    }

    #[test]
    fn reads_cropped_scaled_and_nodata_sources() -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory("complex")?;
        let source = directory.join("source.tif");
        let vrt = directory.join("complex.vrt");
        write_source(&source)?;
        fs::write(
            &vrt,
            vrt_xml(
                4,
                2,
                &format!(
                    "<NoDataValue>999</NoDataValue>{}",
                    complex_source("source.tif", (1, 1, 2, 2), (1, 0, 2, 2))
                ),
            ),
        )?;

        let reader = VrtReader::open(&vrt)?;
        assert_eq!(reader.band_no_data(), Some(999.0));
        assert_eq!(reader.band_sample_type(), RasterSampleType::Float64);
        assert_eq!(
            reader.read_window(WindowRequest {
                x: 0,
                y: 0,
                width: 4,
                height: 2,
                overview: 0,
            })?,
            vec![0.0, 0.0, 13.0, 0.0, 0.0, 19.0, 21.0, 0.0]
        );
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn reads_multiple_sources_into_separate_destination_rectangles()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory("multi-source")?;
        let source = directory.join("source.tif");
        let vrt = directory.join("multi.vrt");
        write_source(&source)?;
        let left = complex_source("source.tif", (0, 0, 2, 2), (0, 0, 2, 2));
        let right = complex_source("source.tif", (2, 0, 2, 2), (2, 0, 2, 2));
        fs::write(&vrt, vrt_xml(4, 2, &format!("{left}{right}")))?;

        let reader = VrtReader::open(&vrt)?;
        assert_eq!(
            reader.read_window(WindowRequest {
                x: 0,
                y: 0,
                width: 4,
                height: 1,
                overview: 0,
            })?,
            vec![1.0, 3.0, 5.0, 7.0]
        );
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn rejects_invalid_missing_recursed_and_unsupported_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory("errors")?;
        let source = directory.join("source.tif");
        write_source(&source)?;

        let malformed = directory.join("malformed.vrt");
        fs::write(
            &malformed,
            r#"<VRTDataset rasterXSize="1" rasterYSize="1">
  <SRS>EPSG:4326</SRS>
  <GeoTransform>1, 1, 0, 1, 0</GeoTransform>
</VRTDataset>"#,
        )?;
        assert!(matches!(
            VrtReader::open(&malformed),
            Err(CtbError::RasterRead(_))
        ));

        let missing = directory.join("missing.vrt");
        fs::write(
            &missing,
            vrt_xml(
                1,
                1,
                &complex_source("missing.tif", (0, 0, 1, 1), (0, 0, 1, 1)),
            ),
        )?;
        assert!(matches!(
            VrtReader::open(&missing),
            Err(CtbError::RasterRead(_) | CtbError::UnsupportedRaster(_))
        ));

        let non_geotiff_path = directory.join("source.png");
        fs::write(&non_geotiff_path, [0x89, b'P', b'N', b'G'])?;
        let non_geotiff = directory.join("non-geotiff.vrt");
        fs::write(
            &non_geotiff,
            vrt_xml(
                1,
                1,
                &complex_source("source.png", (0, 0, 1, 1), (0, 0, 1, 1)),
            ),
        )?;
        assert!(matches!(
            VrtReader::open(&non_geotiff),
            Err(CtbError::UnsupportedRaster(_))
        ));

        let outer = directory.join("outer.vrt");
        let inner = directory.join("inner.vrt");
        fs::write(
            &inner,
            vrt_xml(
                1,
                1,
                &complex_source("outer.vrt", (0, 0, 1, 1), (0, 0, 1, 1)),
            ),
        )?;
        fs::write(
            &outer,
            vrt_xml(
                1,
                1,
                &complex_source("inner.vrt", (0, 0, 1, 1), (0, 0, 1, 1)),
            ),
        )?;
        assert!(matches!(
            VrtReader::open(&outer),
            Err(CtbError::UnsupportedRaster(_))
        ));
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    #[test]
    fn rejects_warped_vrts_and_pixel_functions_before_reading_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = temporary_directory("unsupported-features")?;
        let warped = directory.join("warped.vrt");
        fs::write(
            &warped,
            r#"<VRTDataset rasterXSize="1" rasterYSize="1" subClass="VRTWarpedDataset">
  <SRS>EPSG:4326</SRS>
  <GeoTransform>1, 1, 0, 1, 0, -1</GeoTransform>
  <GDALWarpOptions />
</VRTDataset>"#,
        )?;
        assert!(matches!(
            VrtReader::open(&warped),
            Err(CtbError::UnsupportedRaster(_))
        ));

        let pixel_function = directory.join("pixel-function.vrt");
        fs::write(
            &pixel_function,
            vrt_xml(1, 1, "<PixelFunctionType>inv</PixelFunctionType>"),
        )?;
        assert!(matches!(
            VrtReader::open(&pixel_function),
            Err(CtbError::UnsupportedRaster(_))
        ));
        fs::remove_dir_all(directory)?;
        Ok(())
    }
}
