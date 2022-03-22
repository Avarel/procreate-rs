mod composite;
mod error;
mod ns_archive;

use image::{imageops, Pixel, Rgba, RgbaImage};
use lzokay::decompress::decompress;
use ns_archive::{NsDecode, NsKeyedArchive};
use once_cell::sync::OnceCell;
use plist::{Dictionary, Value};
use regex::Regex;
use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::{Cursor, Read},
};
use zip::read::ZipArchive;

use crate::ns_archive::{NsArchiveError, NsClass, Size, WrappedArray};

type Rgba8 = Rgba<u8>;

fn main() -> Result<(), Box<dyn Error>> {
    let mut archive = ZipArchive::new(
        OpenOptions::new()
            .read(true)
            .write(false)
            .open("./Gilvana.procreate")
            .unwrap(),
    )?;

    let mut document = archive.by_name("Document.archive")?;

    let mut buf = Vec::with_capacity(document.size() as usize);
    document.read_to_end(&mut buf)?;

    drop(document);

    let nka: NsKeyedArchive = plist::from_reader(Cursor::new(buf))?;

    let mut art = ProcreateFile::from_ns(archive, nka)?;
    let mut composite = RgbaImage::new(art.size.width, art.size.height);

    render(&mut composite, &mut art.layers);

    composite.save("./out/final.png")?;

    art.composite.image.unwrap().save("./out/reference.png")?;
    Ok(())
}

fn render(composite: &mut RgbaImage, layers: &SilicaGroup) {
    let mut mask: Option<RgbaImage> = None;

    for layer in layers.children.iter().rev() {
        match layer {
            SilicaLayers::Group(group) => {
                if group.hidden {
                    eprintln!("Hidden group {:?}", group.name);
                    continue;
                }
                render(composite, group);
                eprintln!("Finished group {}", group.name);
            }
            SilicaLayers::Layer(layer) => {
                if layer.hidden {
                    eprintln!("Hidden layer {:?}", layer.name);
                    continue;
                }

                let mut layer_image = layer.image.clone().unwrap();

                if layer.clipped {
                    if let Some(mask) = &mask {
                        composite::layer_clip(&mut layer_image, &mask, layer.opacity);
                    }
                }

                composite::layer_blend(
                    composite,
                    &layer_image,
                    layer.opacity,
                    match layer.blend {
                        1 => composite::multiply,
                        2 => composite::screen,
                        11 => composite::overlay,
                        0 | _ => composite::normal,
                    },
                );

                if !layer.clipped {
                    mask = Some(layer_image);
                }

                eprintln!("Finished layer {:?}: {}", layer.name, layer.blend);
            }
        }
    }
}

struct TilingMeta {
    columns: u32,
    rows: u32,
    diff: Size,
    tile_size: u32,
}

struct ProcreateFile {
    // animation:ValkyrieDocumentAnimation?
    author_name: Option<String>,
    //     backgroundColor:Data?
    // backgroundHidden:Bool?
    //     backgroundColorHSBA:Data?
    //     closedCleanlyKey:Bool?
    //     colorProfile:ValkyrieColorProfile?
    //     composite:SilicaLayer?
    // //  public var drawingguide
    //     faceBackgroundHidden:Bool?
    //     featureSet:Int? = 1
    //     flippedHorizontally:Bool?
    //     flippedVertically:Bool?
    //     isFirstItemAnimationForeground:Bool?
    //     isLastItemAnimationBackground:Bool?
    // //  public var lastTextStyling
    //     layers:[SilicaLayer]?
    //     mask:SilicaLayer?
    //     name:String?
    //     orientation:Int?
    //     primaryItem:Any?
    // //  skipping a bunch of reference window related stuff here
    //     selectedLayer:Any?
    //     selectedSamplerLayer:SilicaLayer?
    //     SilicaDocumentArchiveDPIKey:Float?
    //     SilicaDocumentArchiveUnitKey:Int?
    //     SilicaDocumentTrackedTimeKey:Float?
    //     SilicaDocumentVideoPurgedKey:Bool?
    //     SilicaDocumentVideoSegmentInfoKey:VideoSegmentInfo? // not finished
    //     size: CGSize?
    //     solo: SilicaLayer?
    //     strokeCount: Int?
    //     tileSize: Int?
    //     videoEnabled: Bool? = true
    //     videoQualityKey: String?
    //     videoResolutionKey: String?
    //     videoDuration: String? = "Calculating..."
    composite: SilicaLayer,
    size: Size,
    layers: SilicaGroup,
}

impl ProcreateFile {
    pub fn from_ns(
        mut archive: ZipArchive<File>,
        nka: NsKeyedArchive,
    ) -> Result<Self, NsArchiveError> {
        let root = nka.decode::<&'_ Dictionary>(&nka.top, "root")?;

        println!("{root:#?}");
        let mut layers = nka
            .decode::<WrappedArray<SilicaLayers>>(root, "unwrappedLayers")?
            .objects;

        let file_names = archive.file_names().map(str::to_owned).collect::<Vec<_>>();

        let size = nka.decode::<Size>(root, "size")?;
        let tile_size = nka.decode::<u32>(root, "tileSize")?;
        let columns = size.width / tile_size + if size.width % tile_size == 0 { 0 } else { 1 };
        let rows = size.height / tile_size + if size.height % tile_size == 0 { 0 } else { 1 };

        let meta = TilingMeta {
            columns,
            rows,
            diff: Size {
                width: columns * tile_size - size.width,
                height: rows * tile_size - size.height,
            },
            tile_size,
        };

        // let mut composite = SilicaLayer::from_ns(&nka, nka.decode(root, "composite")?)?;
        let mut composite = nka.decode::<SilicaLayer>(root, "composite")?;
        composite.load_image(&meta, &mut archive, &file_names);

        layers.iter_mut().for_each(|v| {
            v.apply(&mut (|layer| layer.load_image(&meta, &mut archive, &file_names)))
        });

        Ok(Self {
            author_name: nka.decode::<Option<String>>(root, "authorName")?,
            size,
            composite,
            layers: SilicaGroup {
                hidden: false,
                name: "ROOT".to_owned(),
                children: layers,
            },
        })
    }
}

struct SilicaLayer {
    // animationHeldLength:Int?
    blend: u32,
    // bundledImagePath:String?
    // bundledMaskPath:String?
    // bundledVideoPath:String?
    clipped: bool,
    // contentsRect:Data?
    // contentsRectValid:Bool?
    // document:SilicaDocument?
    // extendedBlend:Int?
    hidden: bool,
    // locked:Bool?
    mask: Option<Box<SilicaLayer>>,
    name: Option<String>,
    opacity: f32,
    // perspectiveAssisted:Bool?
    // preserve:Bool?
    // private:Bool?
    // text:ValkyrieText?
    // textPDF:Data?
    // transform:Data?
    // type:Int?
    size_width: u32,
    size_height: u32,
    uuid: String,
    version: u64,
    image: Option<RgbaImage>,
}

impl std::fmt::Debug for SilicaLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SilicaLayer")
            .field("blend", &self.blend)
            .field("clipped", &self.clipped)
            .field("hidden", &self.hidden)
            .field("mask", &self.mask)
            .field("name", &self.name)
            .field("opacity", &self.opacity)
            .field("size_width", &self.size_width)
            .field("size_height", &self.size_height)
            .field("uuid", &self.uuid)
            .field("version", &self.version)
            .finish()
    }
}

impl SilicaLayer {
    fn load_image(
        &mut self,
        meta: &TilingMeta,
        archive: &mut ZipArchive<File>,
        file_names: &[String],
    ) {
        static INSTANCE: OnceCell<Regex> = OnceCell::new();
        let index_regex = INSTANCE.get_or_init(|| Regex::new("(\\d+)~(\\d+)").unwrap());

        let mut image_layer = RgbaImage::new(self.size_width, self.size_height);

        for path in file_names {
            if !path.starts_with(&self.uuid) {
                continue;
            }

            let chunk_str = &path[self.uuid.len()..path.find('.').unwrap_or(path.len())];
            let captures = index_regex.captures(&chunk_str).unwrap();
            let col = u32::from_str_radix(captures.get(1).unwrap().as_str(), 10).unwrap();
            let row = u32::from_str_radix(captures.get(2).unwrap().as_str(), 10).unwrap();

            let tile_width = meta.tile_size
                - if col != meta.columns - 1 {
                    0
                } else {
                    meta.diff.width
                };
            let tile_height = meta.tile_size
                - if row != meta.rows - 1 {
                    0
                } else {
                    meta.diff.height
                };

            let mut chunk = archive.by_name(path).unwrap();
            let mut buf = Vec::new();
            chunk.read_to_end(&mut buf).unwrap();
            // RGBA = 4 channels of 8 bits each, lzo decompressed to lzo data
            let mut dst =
                vec![0; (tile_width * tile_height * u32::from(Rgba8::CHANNEL_COUNT)) as usize];
            decompress(&buf, &mut dst).unwrap();
            let chunked_image = RgbaImage::from_vec(tile_width, tile_height, dst).unwrap();
            imageops::replace(
                &mut image_layer,
                &chunked_image,
                (col * meta.tile_size) as i64,
                (row * meta.tile_size) as i64,
            );
        }

        // image_layer
        //     .par_chunks_exact_mut(usize::from(Rgba8::CHANNEL_COUNT))
        //     .map(Rgba8::from_slice_mut)
        //     .for_each(|pixel| pixel[3] = (f32::from(pixel[3]) * self.opacity) as u8);

        self.image = Some(image_layer);
    }
}

impl NsDecode<'_> for SilicaLayer {
    fn decode(nka: &NsKeyedArchive, val: Option<&Value>) -> Result<Self, NsArchiveError> {
        let coder = <&'_ Dictionary>::decode(nka, val)?;
        Ok(Self {
            blend: nka.decode::<u32>(coder, "blend")?,
            clipped: nka.decode::<bool>(coder, "clipped")?,
            hidden: nka.decode::<bool>(coder, "hidden")?,
            mask: None,
            name: nka.decode::<Option<String>>(coder, "name")?,
            opacity: nka.decode::<f32>(coder, "opacity")?,
            uuid: nka.decode::<String>(coder, "UUID")?,
            version: nka.decode::<u64>(coder, "version")?,
            size_width: nka.decode::<u32>(coder, "sizeWidth")?,
            size_height: nka.decode::<u32>(coder, "sizeHeight")?,
            image: None,
        })
    }
}

#[derive(Debug)]
struct SilicaGroup {
    pub hidden: bool,
    pub children: Vec<SilicaLayers>,
    pub name: String,
}

impl NsDecode<'_> for SilicaGroup {
    fn decode(nka: &NsKeyedArchive, val: Option<&Value>) -> Result<Self, NsArchiveError> {
        let coder = <&'_ Dictionary>::decode(nka, val)?;
        Ok(Self {
            hidden: nka.decode::<bool>(coder, "isHidden")?,
            name: nka.decode::<String>(coder, "name")?,
            children: nka
                .decode::<WrappedArray<SilicaLayers>>(coder, "children")?
                .objects,
        })
    }
}

#[derive(Debug)]
enum SilicaLayers {
    Layer(SilicaLayer),
    Group(SilicaGroup),
}

impl SilicaLayers {
    pub fn apply(&mut self, f: &mut impl FnMut(&mut SilicaLayer)) {
        match self {
            Self::Layer(layer) => f(layer),
            Self::Group(group) => group.children.iter_mut().for_each(|child| child.apply(f)),
        }
    }
}

impl NsDecode<'_> for SilicaLayers {
    fn decode(nka: &NsKeyedArchive, val: Option<&Value>) -> Result<Self, NsArchiveError> {
        let coder = <&'_ Dictionary>::decode(nka, val)?;
        let class = nka.decode::<NsClass>(coder, "$class")?;

        match class.class_name.as_str() {
            "SilicaGroup" => Ok(Self::Group(SilicaGroup::decode(nka, val)?)),
            "SilicaLayer" => Ok(Self::Layer(SilicaLayer::decode(nka, val)?)),
            _ => Err(NsArchiveError::TypeMismatch),
        }
    }
}
