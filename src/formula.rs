use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use crossterm::cursor::MoveTo;
use crossterm::execute;
use image::ImageReader;
use ratatui::layout::Rect;
use ratatui::style::Color;
use serde_json::Value;

use crate::kitty::{self, Placement};
use crate::semantic::SearchHit;
use crate::{cli, metadata};

const RENDER_PPI: f64 = 300.0;
const CROP_PADDING_POINTS: f64 = 1.0;
const IMAGE_Z_INDEX: i32 = 1;
const MAX_CACHED_IMAGES: usize = 64;

#[derive(Clone, Debug)]
pub struct ResolvedFormula {
    pub latex: String,
    pdf_path: PathBuf,
    page: u32,
    crop: PixelCrop,
}

impl ResolvedFormula {
    pub fn pixel_width(&self) -> u32 {
        self.crop.width
    }

    pub fn pixel_height(&self) -> u32 {
        self.crop.height
    }
}

#[derive(Clone, Debug)]
pub struct FormulaOverlay {
    pub formula: ResolvedFormula,
    pub area: Rect,
}

#[derive(Clone, Copy, Debug)]
struct BoundingBox {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PixelCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Debug)]
struct CatalogFormula {
    text: String,
    page: u32,
    bbox: BoundingBox,
    page_width: f64,
    page_height: f64,
}

#[derive(Debug)]
struct RenderedFormula {
    compressed_rgba: Vec<u8>,
    width: u32,
    height: u32,
}

#[derive(Default)]
pub struct FormulaRenderer {
    catalogs: HashMap<PathBuf, Option<Vec<CatalogFormula>>>,
    images: HashMap<String, RenderedFormula>,
    visible_ids: Vec<u32>,
    next_image_id: u32,
    pdftoppm_available: bool,
}

impl FormulaRenderer {
    pub fn new() -> Self {
        Self {
            next_image_id: 0x4752_0000,
            pdftoppm_available: Command::new("pdftoppm")
                .arg("-v")
                .output()
                .is_ok_and(|output| output.status.success()),
            ..Self::default()
        }
    }

    pub fn resolve_hit(&mut self, library: &Path, hit: &SearchHit) -> Vec<Option<ResolvedFormula>> {
        let formulas = extract_formula_texts(&hit.text);
        if formulas.is_empty() {
            return Vec::new();
        }
        let reference_dir = library.join(&hit.dir_name);
        let source = library.join(&hit.source_path);
        if !source.starts_with(&reference_dir) {
            return formulas.into_iter().map(|_| None).collect();
        }
        let Some(document_path) = source.parent().map(|parent| parent.join("document.json")) else {
            return formulas.into_iter().map(|_| None).collect();
        };
        if !document_path.is_file() {
            return formulas.into_iter().map(|_| None).collect();
        }
        let pdf_path = metadata::read_info(&reference_dir)
            .ok()
            .and_then(|reference| cli::find_pdf(&reference_dir, &reference));
        let Some(pdf_path) = pdf_path else {
            return formulas.into_iter().map(|_| None).collect();
        };

        let catalog = self
            .catalogs
            .entry(document_path.clone())
            .or_insert_with(|| load_catalog(&document_path).ok());
        let Some(catalog) = catalog.as_ref() else {
            return formulas.into_iter().map(|_| None).collect();
        };

        let mut used = HashSet::new();
        formulas
            .into_iter()
            .map(|latex| {
                let position = catalog.iter().enumerate().position(|(index, formula)| {
                    !used.contains(&index)
                        && formula.text.trim() == latex.trim()
                        && (hit.pages.is_empty() || hit.pages.contains(&formula.page))
                })?;
                used.insert(position);
                let formula = &catalog[position];
                let crop = pixel_crop(
                    formula.bbox,
                    formula.page_width,
                    formula.page_height,
                    RENDER_PPI,
                )?;
                Some(ResolvedFormula {
                    latex,
                    pdf_path: pdf_path.clone(),
                    page: formula.page,
                    crop,
                })
            })
            .collect()
    }

    pub fn enabled_for(&self, text: Color, background: Color) -> bool {
        self.pdftoppm_available
            && kitty_supported()
            && color_rgb(text).is_some()
            && color_rgb(background).is_some()
    }

    pub fn render(
        &mut self,
        output: &mut impl Write,
        overlays: &[FormulaOverlay],
        text: Color,
        background: Color,
    ) {
        for image_id in self.visible_ids.drain(..) {
            let _ = kitty::delete_image(output, image_id);
        }
        if overlays.is_empty() || !kitty_supported() {
            return;
        }
        let (Some(text), Some(background)) = (color_rgb(text), color_rgb(background)) else {
            return;
        };

        for overlay in overlays {
            let key = render_key(&overlay.formula, text, background);
            if !self.images.contains_key(&key) {
                if self.images.len() >= MAX_CACHED_IMAGES {
                    self.images.clear();
                }
                let Ok(image) = render_formula(&overlay.formula, text, background) else {
                    continue;
                };
                self.images.insert(key.clone(), image);
            }
            let Some(image) = self.images.get(&key) else {
                continue;
            };
            self.next_image_id = self.next_image_id.wrapping_add(1).max(1);
            let image_id = self.next_image_id;
            if execute!(output, MoveTo(overlay.area.x, overlay.area.y)).is_err() {
                continue;
            }
            if kitty::transmit_compressed_rgba(
                output,
                &image.compressed_rgba,
                image.width,
                image.height,
                Placement {
                    image_id,
                    columns: overlay.area.width,
                    rows: overlay.area.height,
                    z_index: IMAGE_Z_INDEX,
                },
            )
            .is_ok()
            {
                self.visible_ids.push(image_id);
            }
        }
    }

    pub fn clear(&mut self, output: &mut impl Write) {
        for image_id in self.visible_ids.drain(..) {
            let _ = kitty::delete_image(output, image_id);
        }
    }
}

pub fn extract_formula_texts(text: &str) -> Vec<String> {
    let mut formulas = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        match (line.trim(), current.as_mut()) {
            ("\\[" | "$$", None) => current = Some(Vec::new()),
            ("\\]" | "$$", Some(lines)) => {
                let formula = lines.join("\n").trim().to_string();
                if !formula.is_empty() {
                    formulas.push(formula);
                }
                current = None;
            }
            (_, Some(lines)) => lines.push(line),
            _ => {}
        }
    }
    formulas
}

fn load_catalog(path: &Path) -> Result<Vec<CatalogFormula>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read formula provenance from {}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)?;
    let texts = document
        .get("texts")
        .and_then(Value::as_array)
        .context("Docling document has no texts")?;
    let mut formulas = Vec::new();
    for item in texts {
        if item.get("label").and_then(Value::as_str) != Some("formula") {
            continue;
        }
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };
        let Some(page) = item
            .pointer("/prov/0/page_no")
            .and_then(Value::as_u64)
            .and_then(|page| u32::try_from(page).ok())
        else {
            continue;
        };
        let Some(bbox) = parse_bbox(item.pointer("/prov/0/bbox")) else {
            continue;
        };
        if item
            .pointer("/prov/0/bbox/coord_origin")
            .and_then(Value::as_str)
            != Some("BOTTOMLEFT")
        {
            continue;
        }
        let page_key = page.to_string();
        let Some(page_width) = document
            .pointer(&format!("/pages/{page_key}/size/width"))
            .and_then(Value::as_f64)
        else {
            continue;
        };
        let Some(page_height) = document
            .pointer(&format!("/pages/{page_key}/size/height"))
            .and_then(Value::as_f64)
        else {
            continue;
        };
        formulas.push(CatalogFormula {
            text: text.to_string(),
            page,
            bbox,
            page_width,
            page_height,
        });
    }
    Ok(formulas)
}

fn parse_bbox(value: Option<&Value>) -> Option<BoundingBox> {
    let value = value?;
    let bbox = BoundingBox {
        left: value.get("l")?.as_f64()?,
        top: value.get("t")?.as_f64()?,
        right: value.get("r")?.as_f64()?,
        bottom: value.get("b")?.as_f64()?,
    };
    (bbox.left.is_finite()
        && bbox.top.is_finite()
        && bbox.right.is_finite()
        && bbox.bottom.is_finite())
    .then_some(bbox)
}

fn pixel_crop(bbox: BoundingBox, page_width: f64, page_height: f64, ppi: f64) -> Option<PixelCrop> {
    if page_width <= 0.0 || page_height <= 0.0 || bbox.right <= bbox.left || bbox.top <= bbox.bottom
    {
        return None;
    }
    let scale = ppi / 72.0;
    let left = (bbox.left - CROP_PADDING_POINTS).max(0.0);
    let top = (page_height - bbox.top - CROP_PADDING_POINTS).max(0.0);
    let right = (bbox.right + CROP_PADDING_POINTS).min(page_width);
    let bottom = (page_height - bbox.bottom + CROP_PADDING_POINTS).min(page_height);
    Some(PixelCrop {
        x: (left * scale).floor() as u32,
        y: (top * scale).floor() as u32,
        width: ((right - left) * scale).ceil().max(1.0) as u32,
        height: ((bottom - top) * scale).ceil().max(1.0) as u32,
    })
}

fn render_formula(
    formula: &ResolvedFormula,
    text: (u8, u8, u8),
    background: (u8, u8, u8),
) -> Result<RenderedFormula> {
    let temporary = tempfile::tempdir()?;
    let prefix = temporary.path().join("formula");
    let output = Command::new("pdftoppm")
        .args([
            "-png",
            "-singlefile",
            "-q",
            "-aa",
            "yes",
            "-aaVector",
            "yes",
        ])
        .arg("-f")
        .arg(formula.page.to_string())
        .arg("-l")
        .arg(formula.page.to_string())
        .arg("-r")
        .arg(RENDER_PPI.to_string())
        .arg("-x")
        .arg(formula.crop.x.to_string())
        .arg("-y")
        .arg(formula.crop.y.to_string())
        .arg("-W")
        .arg(formula.crop.width.to_string())
        .arg("-H")
        .arg(formula.crop.height.to_string())
        .arg(&formula.pdf_path)
        .arg(&prefix)
        .output()
        .context("Failed to run pdftoppm for formula rendering")?;
    anyhow::ensure!(
        output.status.success(),
        "pdftoppm could not render formula on page {}: {}",
        formula.page,
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let path = prefix.with_extension("png");
    let mut image = ImageReader::open(&path)?.decode()?.into_rgba8();
    let (width, height) = image.dimensions();
    let coverage = image
        .pixels()
        .map(|pixel| emphasize_coverage(ink_coverage(pixel[0], pixel[1], pixel[2])))
        .collect::<Vec<_>>();
    for (pixel, coverage) in image.pixels_mut().zip(coverage) {
        pixel[0] = blend(background.0, text.0, coverage);
        pixel[1] = blend(background.1, text.1, coverage);
        pixel[2] = blend(background.2, text.2, coverage);
        pixel[3] = u8::MAX;
    }
    Ok(RenderedFormula {
        compressed_rgba: kitty::compress_rgba(image.as_raw())?,
        width,
        height,
    })
}

fn ink_coverage(red: u8, green: u8, blue: u8) -> u16 {
    let luminance =
        (77 * u32::from(red) + 150 * u32::from(green) + 29 * u32::from(blue) + 128) / 256;
    u16::try_from(255_u32.saturating_sub(luminance)).unwrap_or_default()
}

fn emphasize_coverage(coverage: u16) -> u16 {
    coverage.saturating_mul(5).div_ceil(4).min(255)
}

fn blend(background: u8, foreground: u8, coverage: u16) -> u8 {
    let inverse = 255_u16.saturating_sub(coverage);
    ((u16::from(background) * inverse + u16::from(foreground) * coverage) / 255) as u8
}

fn render_key(formula: &ResolvedFormula, text: (u8, u8, u8), background: (u8, u8, u8)) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}:{}:{text:?}:{background:?}",
        formula.pdf_path.display(),
        formula.page,
        formula.crop.x,
        formula.crop.y,
        formula.crop.width,
        formula.crop.height,
        formula.latex
    )
}

fn kitty_supported() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM").is_ok_and(|term| term.contains("kitty"))
}

fn color_rgb(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((205, 49, 49)),
        Color::Green => Some((13, 188, 121)),
        Color::Yellow => Some((229, 229, 16)),
        Color::Blue => Some((36, 114, 200)),
        Color::Magenta => Some((188, 63, 188)),
        Color::Cyan => Some((17, 168, 205)),
        Color::Gray => Some((204, 204, 204)),
        Color::DarkGray => Some((102, 102, 102)),
        Color::LightRed => Some((241, 76, 76)),
        Color::LightGreen => Some((35, 209, 139)),
        Color::LightYellow => Some((245, 245, 67)),
        Color::LightBlue => Some((59, 142, 234)),
        Color::LightMagenta => Some((214, 112, 214)),
        Color::LightCyan => Some((41, 184, 219)),
        Color::White => Some((242, 242, 242)),
        Color::Reset | Color::Indexed(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoundingBox, blend, emphasize_coverage, extract_formula_texts, ink_coverage, load_catalog,
        pixel_crop,
    };

    #[test]
    fn extracts_display_formula_blocks_in_order() {
        let text = "Before\n\n\\[\na_n \\to L\n\\]\n\nMiddle\n\n$$\nb_n = 1\n$$";
        assert_eq!(extract_formula_texts(text), ["a_n \\to L", "b_n = 1"]);
    }

    #[test]
    fn converts_bottom_left_points_to_render_pixels() {
        let crop = pixel_crop(
            BoundingBox {
                left: 100.0,
                top: 200.0,
                right: 200.0,
                bottom: 180.0,
            },
            400.0,
            600.0,
            216.0,
        )
        .unwrap();
        assert_eq!(crop.x, 297);
        assert_eq!(crop.y, 1197);
        assert_eq!(crop.width, 306);
        assert_eq!(crop.height, 66);
    }

    #[test]
    fn loads_docling_formula_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.json");
        std::fs::write(
            &path,
            r#"{
                "pages":{"3":{"size":{"width":400.0,"height":600.0}}},
                "texts":[{
                    "label":"formula","text":"a_n \\to L",
                    "prov":[{"page_no":3,"bbox":{"l":100.0,"t":200.0,"r":200.0,"b":180.0,"coord_origin":"BOTTOMLEFT"}}]
                }]
            }"#,
        )
        .unwrap();
        let catalog = load_catalog(&path).unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].text, "a_n \\to L");
        assert_eq!(catalog[0].page, 3);
    }

    #[test]
    fn composites_formula_ink_over_the_theme_background() {
        assert_eq!(blend(10, 210, 0), 10);
        assert_eq!(blend(10, 210, 255), 210);
    }

    #[test]
    fn derives_formula_coverage_from_the_white_pdf_raster() {
        assert_eq!(ink_coverage(255, 255, 255), 0);
        assert_eq!(ink_coverage(0, 0, 0), 255);
        assert!(ink_coverage(128, 128, 128).abs_diff(127) <= 1);
    }

    #[test]
    fn strengthens_formula_ink_without_changing_its_shape() {
        assert_eq!(emphasize_coverage(0), 0);
        assert_eq!(emphasize_coverage(100), 125);
        assert_eq!(emphasize_coverage(220), 255);
        assert_eq!(emphasize_coverage(255), 255);
    }
}
