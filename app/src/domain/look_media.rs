//! Look pictures and files made on the phone: previews of a sample photo,
//! LUT and Lightroom preset imports, and renamed or copied GRF effects.
//!
//! Previews follow the site: the effect's RAW preview table (`site`) applied
//! to a scene-linear sample with trilinear interpolation (`web/src/preview`).

use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result, ensure};
use grkit_core::{effects, fit, site};

/// The largest LUT file the phone converts (the site's limit).
pub const MAX_LUT_SIZE: usize = 128 * 1024 * 1024;
/// A thumbnail is the preview reduced by this factor on each side.
const THUMB_FACTOR: usize = 3;

/// Scene-linear sample photo, `GRLIN001`: magic, LE u32 width and height,
/// then LE u16 RGB. A 600 x 400 reduction of the site's sample.
const SAMPLE: &[u8] = include_bytes!("../../data/look-sample.grlin");

/// The camera's base gamma for demo previews (GR III 2.21 site catalog).
pub const DEMO_BASE_GAMMA: &str = include_str!("../../data/demo-base-gamma.json");

/// Straight 8-bit RGBA pixels, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

/// A Look shown on a sample photo, full size and as a thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookPreview {
    pub full: Picture,
    pub thumb: Picture,
}

struct Linear {
    width: usize,
    height: usize,
    data: Vec<u16>,
}

fn decode_linear(bytes: &[u8]) -> Result<Linear> {
    ensure!(
        bytes.len() >= 16 && &bytes[..8] == b"GRLIN001",
        "Invalid linear sample"
    );
    let width = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    let height = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
    ensure!(
        width > 0 && height > 0 && bytes.len() == 16 + width * height * 6,
        "Invalid linear sample size"
    );
    let data = bytes[16..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Ok(Linear {
        width,
        height,
        data,
    })
}

fn sample() -> &'static Linear {
    static SAMPLE_PICTURE: OnceLock<Linear> = OnceLock::new();
    SAMPLE_PICTURE.get_or_init(|| decode_linear(SAMPLE).expect("bundled sample decodes"))
}

/// The demo base gamma table.
pub fn demo_base_gamma() -> Vec<u16> {
    serde_json::from_str(DEMO_BASE_GAMMA).expect("bundled base gamma parses")
}

/// The sRGB transfer, used by the table grid as a shaper.
fn srgb_coordinate(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Apply an 8-bit RGB table of `size` points per axis (red fastest).
fn apply_table(image: &Linear, table: &[u8], size: usize) -> Result<Vec<u8>> {
    ensure!(
        (2..=256).contains(&size) && table.len() == size * size * size * 3,
        "Preview table size mismatch"
    );
    let stride = [3, size * 3, size * size * 3];
    // Per input level: the lower grid index and the weight of the upper one.
    let mut low = vec![0u16; 65536];
    let mut weight = vec![0f32; 65536];
    for v in 0..65536usize {
        let x = srgb_coordinate(v as f64 / 65535.0) * (size - 1) as f64;
        let i = (x.floor() as usize).min(size - 2);
        low[v] = i as u16;
        weight[v] = (x - i as f64) as f32;
    }
    let pixels = image.width * image.height;
    let mut out = vec![0u8; pixels * 4];
    let t = |i: usize| table[i] as f32;
    for p in 0..pixels {
        let r = image.data[p * 3] as usize;
        let g = image.data[p * 3 + 1] as usize;
        let b = image.data[p * 3 + 2] as usize;
        let (fr, fg, fb) = (weight[r], weight[g], weight[b]);
        let base =
            low[r] as usize * stride[0] + low[g] as usize * stride[1] + low[b] as usize * stride[2];
        for c in 0..3 {
            let o = base + c;
            let c00 = t(o) + (t(o + stride[0]) - t(o)) * fr;
            let c10 = t(o + stride[1]) + (t(o + stride[1] + stride[0]) - t(o + stride[1])) * fr;
            let c01 = t(o + stride[2]) + (t(o + stride[2] + stride[0]) - t(o + stride[2])) * fr;
            let c11 = t(o + stride[2] + stride[1])
                + (t(o + stride[2] + stride[1] + stride[0]) - t(o + stride[2] + stride[1])) * fr;
            let c0 = c00 + (c10 - c00) * fg;
            let c1 = c01 + (c11 - c01) * fg;
            out[p * 4 + c] = (c0 + (c1 - c0) * fb).round().clamp(0.0, 255.0) as u8;
        }
        out[p * 4 + 3] = 255;
    }
    Ok(out)
}

/// Average `factor` x `factor` blocks.
fn reduce(rgba: &[u8], width: usize, height: usize, factor: usize) -> (usize, usize, Vec<u8>) {
    let (w, h) = (width / factor, height / factor);
    let mut out = vec![0u8; w * h * 4];
    let area = (factor * factor) as u32;
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let mut sum = 0u32;
                for dy in 0..factor {
                    for dx in 0..factor {
                        sum += rgba[((y * factor + dy) * width + x * factor + dx) * 4 + c] as u32;
                    }
                }
                out[(y * w + x) * 4 + c] = ((sum + area / 2) / area) as u8;
            }
        }
    }
    (w, h, out)
}

/// The sample photo in a GRF effect, over the camera's `base_gamma`.
pub fn render_preview(grf: &[u8], base_gamma: &[u16]) -> Result<LookPreview> {
    let base_gamma = site::base_gamma(base_gamma)?;
    let (effect, _) = effects::decode(grf)?;
    let table = site::native_preview_lut(&base_gamma, &effect, None)?;
    let image = sample();
    let rgba = apply_table(image, &table, site::LUT_SIZE)?;
    let (tw, th, thumb) = reduce(&rgba, image.width, image.height, THUMB_FACTOR);
    Ok(LookPreview {
        full: Picture {
            width: image.width as u32,
            height: image.height as u32,
            rgba: rgba.into(),
        },
        thumb: Picture {
            width: tw as u32,
            height: th as u32,
            rgba: thumb.into(),
        },
    })
}

/// The 40 x 40 camera menu icon of a GRF effect.
pub fn icon(grf: &[u8]) -> Result<Picture> {
    let (_, icon) = effects::decode(grf)?;
    ensure!(icon.len() == 40 * 40 * 4, "Unexpected icon size");
    Ok(Picture {
        width: 40,
        height: 40,
        rgba: icon.into(),
    })
}

/// Name, ID and monochrome flag of a GRF effect.
pub fn label(grf: &[u8]) -> Result<(String, String, bool)> {
    let (effect, _) = effects::decode(grf)?;
    Ok((
        effect.id,
        effect.name,
        effect.flags & effects::MONOCHROME != 0,
    ))
}

/// Fit a `.cube` file, a LUT image, or a Lightroom `.xmp` preset into a GRF
/// effect with the given ID and name, and a film box cover from the name.
pub fn import(
    bytes: &[u8],
    file_name: &str,
    id: &str,
    name: &str,
    base_gamma: &[u16],
) -> Result<Vec<u8>> {
    ensure!(
        bytes.len() <= MAX_LUT_SIZE,
        "The file is larger than {} MB",
        MAX_LUT_SIZE >> 20
    );
    effects::check_identity(id, name)?;
    let base_gamma = site::base_gamma(base_gamma)?;
    let icon = crate::domain::looks::cover_for_name(name);
    let imported = fit::import(
        bytes,
        &base_gamma,
        &fit::Import {
            effect_id: id,
            name,
            icon: &icon,
            source: file_name,
            input: None,
            exposure: 0.0,
            options: fit::Options::default(),
        },
    )?;
    effects::compile_definition(&effects::parse_json(imported.text.as_bytes())?)
}

/// A GRF effect with a new name and a cover drawn from it. The ID stays, so
/// the camera keeps its Fn adjustments for the Look.
pub fn rename(grf: &[u8], name: &str) -> Result<Vec<u8>> {
    let (mut effect, _) = effects::decode(grf)?;
    effects::check_identity(&effect.id, name)?;
    let icon = effects::icon_pixels(&crate::domain::looks::cover_for_name(name))?;
    effect.name = name.to_owned();
    effect.encode(&icon)
}

/// An editable copy of an official GRF effect under a new ID.
pub fn copy_as(grf: &[u8], id: &str, name: &str) -> Result<Vec<u8>> {
    let (mut effect, _) = effects::decode(grf)?;
    effects::check_identity(id, name).context("The copy needs another name")?;
    let icon = effects::icon_pixels(&crate::domain::looks::cover_for_name(name))?;
    effect.id = id.to_owned();
    effect.name = name.to_owned();
    effect.encode(&icon)
}

/// Compile a bundled effect definition (demo mode).
pub fn compile(definition: &str) -> Result<Vec<u8>> {
    effects::compile_definition(&effects::parse_json(definition.as_bytes())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VELVIA: &str = include_str!("../../../effects/velvia-100-v1/effect.json");

    #[test]
    fn previews_the_sample_in_a_look() {
        let grf = compile(VELVIA).unwrap();
        let preview = render_preview(&grf, &demo_base_gamma()).unwrap();
        assert_eq!((preview.full.width, preview.full.height), (600, 400));
        assert_eq!((preview.thumb.width, preview.thumb.height), (200, 133));
        assert_eq!(preview.thumb.rgba.len(), 200 * 133 * 4);
        // Not a flat picture.
        let first = &preview.full.rgba[..3];
        assert!(preview.full.rgba.chunks(4).any(|p| &p[..3] != first));
    }

    #[test]
    fn identity_table_keeps_values() {
        let size = 2;
        let mut table = Vec::new();
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    table.extend([r as u8 * 255, g as u8 * 255, b as u8 * 255]);
                }
            }
        }
        let image = Linear {
            width: 1,
            height: 1,
            data: vec![65535, 0, 65535],
        };
        assert_eq!(
            apply_table(&image, &table, size).unwrap(),
            [255, 0, 255, 255]
        );
    }

    #[test]
    fn renames_and_copies_keep_valid_effects() {
        let grf = compile(VELVIA).unwrap();
        let renamed = rename(&grf, "My Velvia").unwrap();
        let (id, name, _) = label(&renamed).unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("velvia-100-v1", "My Velvia"));
        let copy = copy_as(&grf, "lut-velvia-100", "Velvia 100").unwrap();
        assert_eq!(label(&copy).unwrap().0, "lut-velvia-100");
        assert_eq!(icon(&copy).unwrap().rgba.len(), 40 * 40 * 4);
    }

    #[test]
    fn imports_a_cube_file() {
        // A 2-point identity cube.
        let cube = "LUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let grf = import(
            cube.as_bytes(),
            "identity.cube",
            "lut-identity",
            "Identity",
            &demo_base_gamma(),
        )
        .unwrap();
        assert_eq!(label(&grf).unwrap().1, "Identity");
        assert!(import(b"not a lut", "x.cube", "lut-x", "X", &demo_base_gamma()).is_err());
    }
}
