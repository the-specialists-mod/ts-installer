//! Player models in `ts/models/player/<name>/`: finding them, their readme and preview picture,
//! choosing one, and adding new ones.
//!
//! TS picks the player model with the engine's own `model` userinfo cvar (archived in
//! `config.cfg` as `model "gordon"`); the engine loads `models/player/<model>/<model>.mdl`. On
//! teamplay servers (team deathmatch, The One) the server replaces it with a model from the team's
//! list (`mp_teammodels` or the map's `teammodels`), and it refuses the HLDM models.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use crate::cfgfile::CfgFile;

/// The userinfo cvar that holds the player model.
pub const MODEL_CVAR: &str = "model";

pub fn player_dir(ts: &Path) -> PathBuf {
    ts.join("models").join("player")
}

/// One folder in `models/player`.
#[derive(Debug, Clone, Default)]
pub struct PlayerModel {
    /// The folder name: the value of the `model` cvar.
    pub name: String,
    pub dir: PathBuf,
    /// `<name>.mdl`; missing means the engine can't load this model.
    pub mdl: Option<PathBuf>,
    pub bmp: Option<PathBuf>,
    pub txt: Option<PathBuf>,
    /// The readme, if there is one.
    pub text: String,
    /// "Model Name" from the readme.
    pub title: Option<String>,
    pub author: Option<String>,
    /// Every file in the folder with its size.
    pub files: Vec<(String, u64)>,
}

impl PlayerModel {
    pub fn read(dir: &Path) -> Self {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut files: Vec<(String, u64)> = std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0)))
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|f| f.0.to_ascii_lowercase());
        // The engine wants `<name>.mdl`; on Windows (and in our lookups) case doesn't matter.
        let find = |want: &dyn Fn(&str) -> bool| files.iter().find(|(f, _)| want(&f.to_ascii_lowercase())).map(|(f, _)| dir.join(f));
        let lower = name.to_ascii_lowercase();
        let mdl = find(&|f| f == format!("{lower}.mdl"));
        let bmp = find(&|f| f == format!("{lower}.bmp")).or_else(|| find(&|f| f.ends_with(".bmp")));
        let txt = find(&|f| f == format!("{lower}.txt")).or_else(|| find(&|f| f.ends_with(".txt")));
        let text = txt.as_ref().and_then(|p| std::fs::read(p).ok()).map(|b| decode_text(&b)).unwrap_or_default();
        let (title, author) = (readme_field(&text, "model name"), readme_field(&text, "author"));
        Self { name, dir: dir.to_path_buf(), mdl, bmp, txt, text, title, author, files }
    }

    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.1).sum()
    }
}

/// The models under `ts/models/player`, sorted by name.
pub fn list(ts: &Path) -> Vec<PlayerModel> {
    let Ok(entries) = std::fs::read_dir(player_dir(ts)) else { return Vec::new() };
    let mut out: Vec<PlayerModel> = entries.flatten().filter(|e| e.path().is_dir()).map(|e| PlayerModel::read(&e.path())).collect();
    out.sort_by_key(|m| m.name.to_ascii_lowercase());
    out
}

/// Readmes are old Windows text: UTF-8 if it is, else Windows-1252 (as Latin-1), CRLF to LF.
fn decode_text(bytes: &[u8]) -> String {
    let s = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    };
    s.replace("\r\n", "\n").replace('\r', "\n").replace('\t', "    ")
}

/// `Author : Someone <mail>` style lines from the HL model readme template.
fn readme_field(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        let v = v.trim();
        (k.trim().eq_ignore_ascii_case(key) && !v.is_empty()).then(|| v.to_owned())
    })
}

/// The model `config.cfg` selects, if it sets one.
pub fn current(ts: &Path) -> Option<String> {
    CfgFile::load(&ts.join("config.cfg")).ok()?.get(MODEL_CVAR).filter(|m| !m.is_empty())
}

/// Sets the `model` cvar in `ts/config.cfg`. The file must exist: without one the engine runs
/// its defaults, which a one-line config.cfg would replace.
pub fn select(ts: &Path, name: &str) -> Result<()> {
    let path = ts.join("config.cfg");
    ensure!(path.is_file(), "{} doesn't exist yet; start the game once first", path.display());
    let mut cfg = CfgFile::load(&path)?;
    cfg.set(MODEL_CVAR, name);
    cfg.save()
}

/// What `add` would install from a folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// The model name: the main `.mdl`'s file stem.
    pub name: String,
    pub files: Vec<PathBuf>,
    /// `models/player/<name>` already exists.
    pub exists: bool,
}

/// Looks at `src` (a model folder, or a file inside one) and works out what to copy. The name is
/// the `.mdl` stem, because the engine loads `<name>/<name>.mdl`; texture (`<name>T.mdl`) and
/// sequence group (`<name>01.mdl`) files come along.
pub fn plan_import(src: &Path, ts: &Path) -> Result<Import> {
    let dir = if src.is_file() { src.parent().context("file has no folder")? } else { src };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| p.extension().is_some_and(|x| ["mdl", "bmp", "txt"].iter().any(|e| x.eq_ignore_ascii_case(e))))
        .collect();
    files.sort();
    let stem = |p: &PathBuf| p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let mdls: Vec<&PathBuf> = files.iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mdl"))).collect();
    // The main model is the one the others are named after (`x.mdl` beside `xT.mdl`, `x01.mdl`).
    let main = mdls
        .iter()
        .find(|m| {
            let s = stem(m).to_ascii_lowercase();
            mdls.iter().all(|o| o.file_stem().unwrap_or_default().to_string_lossy().to_ascii_lowercase().starts_with(&s))
        })
        .copied()
        .or(if mdls.len() == 1 { mdls.first().copied() } else { None });
    let Some(main) = main else {
        if mdls.is_empty() {
            bail!("no .mdl file in {}", dir.display());
        }
        bail!("{} has several unrelated .mdl files; pick a folder with one model", dir.display());
    };
    let data = std::fs::read(main).with_context(|| format!("read {}", main.display()))?;
    crate::mdl::check_header(&data).with_context(|| format!("{} isn't a Half-Life model", main.display()))?;
    let name = stem(main);
    ensure!(
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) && name.len() < 32,
        "\"{name}\" can't be used as a model name (letters, digits, - and _ only, under 32 characters)"
    );
    let exists = player_dir(ts).read_dir().is_ok_and(|d| d.flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&name)));
    Ok(Import { name, files, exists })
}

/// Copies an import into `models/player/<name>/`. An existing folder is only written into with
/// `overwrite`. Returns the destination folder.
pub fn import(plan: &Import, ts: &Path, overwrite: bool) -> Result<PathBuf> {
    let root = player_dir(ts);
    // Reuse an existing folder's spelling so case-sensitive systems don't get two.
    let existing = root
        .read_dir()
        .ok()
        .and_then(|d| d.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&plan.name)))
        .map(|e| e.path());
    ensure!(existing.is_none() || overwrite, "models/player/{} already exists", plan.name);
    let dest = existing.unwrap_or_else(|| root.join(&plan.name));
    std::fs::create_dir_all(&dest).with_context(|| format!("create {}", dest.display()))?;
    for f in &plan.files {
        let to = dest.join(f.file_name().context("file without a name")?);
        if f.canonicalize().ok() == to.canonicalize().ok() && to.exists() {
            continue;
        }
        std::fs::copy(f, &to).with_context(|| format!("copy {} to {}", f.display(), to.display()))?;
    }
    Ok(dest)
}

/// An image as rows of RGBA bytes, top row first.
#[derive(Debug, Clone, PartialEq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Decodes an uncompressed Windows BMP: 1/4/8-bit paletted (what HL's model previews are), or
/// 24/32-bit.
pub fn decode_bmp(data: &[u8]) -> Result<Rgba> {
    let u16_at = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).context("BMP is truncated");
    let u32_at = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).context("BMP is truncated");
    ensure!(data.starts_with(b"BM"), "not a BMP file");
    let pixel_offset = u32_at(10)? as usize;
    let header = u32_at(14)? as usize;
    let (width, height, bits, compression, colors_used) = if header == 12 {
        (u16_at(18)? as i64, u16_at(20)? as i16 as i64, u16_at(24)?, 0, 0)
    } else {
        ensure!(header >= 40, "unknown BMP header size {header}");
        (u32_at(18)? as i32 as i64, u32_at(22)? as i32 as i64, u16_at(28)?, u32_at(30)?, u32_at(46)? as usize)
    };
    // Bitfields with 32 bits are plain BGRA in practice.
    ensure!(compression == 0 || (compression == 3 && bits == 32), "compressed BMPs aren't supported");
    let top_down = height < 0;
    let (width, height) = (width as usize, height.unsigned_abs() as usize);
    ensure!(width > 0 && height > 0 && width <= 8192 && height <= 8192, "bad BMP size {width}x{height}");
    let palette: Vec<[u8; 3]> = if bits <= 8 {
        let entry = if header == 12 { 3 } else { 4 };
        let count = if colors_used == 0 { 1 << bits } else { colors_used.min(256) };
        let start = 14 + header;
        (0..count)
            .map(|i| data.get(start + i * entry..start + i * entry + 3).map(|c| [c[2], c[1], c[0]]).context("BMP palette is truncated"))
            .collect::<Result<_>>()?
    } else {
        Vec::new()
    };
    ensure!(matches!(bits, 1 | 4 | 8 | 24 | 32), "{bits}-bit BMPs aren't supported");
    let stride = (width * bits as usize).div_ceil(32) * 4;
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        let src_row = if top_down { y } else { height - 1 - y };
        let row = data.get(pixel_offset + src_row * stride..pixel_offset + (src_row + 1) * stride).context("BMP pixels are truncated")?;
        for x in 0..width {
            let rgba = match bits {
                24 | 32 => {
                    let p = &row[x * (bits as usize / 8)..];
                    [p[2], p[1], p[0], 255]
                }
                _ => {
                    let per_byte = 8 / bits as usize;
                    let byte = row[x / per_byte];
                    let shift = 8 - bits as usize * (x % per_byte + 1);
                    let index = ((byte >> shift) & ((1u16 << bits) - 1) as u8) as usize;
                    let c = palette.get(index).copied().unwrap_or_default();
                    [c[0], c[1], c[2], 255]
                }
            };
            pixels[(y * width + x) * 4..][..4].copy_from_slice(&rgba);
        }
    }
    Ok(Rgba { width, height, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("target/test-scratch/models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A bottom-up BMP with the given bit depth and rows (bottom row last in `rows`'s order reversed).
    fn bmp(bits: u16, width: u32, height: i32, palette: &[[u8; 4]], body: &[u8]) -> Vec<u8> {
        let offset = 54 + palette.len() as u32 * 4;
        let mut d = Vec::new();
        d.extend_from_slice(b"BM");
        d.extend_from_slice(&(offset + body.len() as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        d.extend_from_slice(&offset.to_le_bytes());
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&width.to_le_bytes());
        d.extend_from_slice(&height.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&bits.to_le_bytes());
        d.extend_from_slice(&[0; 16]);
        d.extend_from_slice(&(palette.len() as u32).to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        for p in palette {
            d.extend_from_slice(p);
        }
        d.extend_from_slice(body);
        d
    }

    #[test]
    fn decodes_8bit_bottom_up() {
        // Palette is BGRx: index 1 is red, 2 is blue. Rows are padded to 4 bytes.
        let pal = [[0, 0, 0, 0], [0, 0, 255, 0], [255, 0, 0, 0]];
        let body = [1, 2, 0, 0, /* top row: */ 2, 1, 0, 0];
        let img = decode_bmp(&bmp(8, 2, 2, &pal, &body)).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.pixels[..8], &[0, 0, 255, 255, 255, 0, 0, 255]);
        assert_eq!(&img.pixels[8..], &[255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn decodes_24bit_top_down() {
        let body = [0, 255, 0, 10, 20, 30, 0, 0];
        let img = decode_bmp(&bmp(24, 2, -1, &[], &body)).unwrap();
        assert_eq!(img.pixels, [0, 255, 0, 255, 30, 20, 10, 255]);
    }

    #[test]
    fn rejects_bad_bmps() {
        assert!(decode_bmp(b"PNG").is_err());
        let mut d = bmp(24, 4, 4, &[], &[0; 8]);
        assert!(decode_bmp(&d).is_err(), "truncated");
        d[30] = 1; // RLE8
        assert!(decode_bmp(&d).unwrap_err().to_string().contains("compressed"));
    }

    #[test]
    fn decodes_shipped_previews() {
        let dir = Path::new(env!("HOME")).join("dev/hl/ts_original");
        let models = list(&dir);
        if models.is_empty() {
            eprintln!("TS 3.0 not found; skipping");
            return;
        }
        let names: Vec<&str> = models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["agent", "castor", "gordon", "hitman", "laurence", "merc", "seal"]);
        for m in &models {
            assert!(m.mdl.is_some(), "{}", m.name);
            let img = decode_bmp(&std::fs::read(m.bmp.as_ref().unwrap()).unwrap()).unwrap();
            assert_eq!((img.width, img.height), (164, 200), "{}", m.name);
            assert!(m.author.is_some(), "{}", m.name);
        }
        let castor = &models[1];
        assert!(castor.txt.as_ref().unwrap().ends_with("CastorTroy.txt"));
        assert_eq!(castor.title.as_deref(), Some("Castor Troy"));
        assert!(castor.author.as_deref().unwrap().starts_with("Antonio \"RAvEN\" Teglia"));
        assert!(!castor.text.contains('\r'));
    }

    #[test]
    fn readme_fields() {
        let t = "Model Name  : Seal\nAuthor : \nAuthor      : Morfeo <x@y>\n";
        assert_eq!(readme_field(t, "model name").as_deref(), Some("Seal"));
        assert_eq!(readme_field(t, "author").as_deref(), Some("Morfeo <x@y>"));
        assert_eq!(decode_text(b"caf\xe9\r\nok"), "café\nok");
    }

    fn fake_mdl() -> Vec<u8> {
        let mut d = vec![0u8; 300];
        d[..4].copy_from_slice(b"IDST");
        d[4] = 10;
        d
    }

    #[test]
    fn selects_model_in_config() {
        let ts = scratch("select");
        assert!(select(&ts, "seal").is_err(), "no config.cfg");
        std::fs::write(ts.join("config.cfg"), "bind \"w\" \"+forward\"\nmodel \"gordon\"\nname \"Player\"\n").unwrap();
        assert_eq!(current(&ts).as_deref(), Some("gordon"));
        select(&ts, "seal").unwrap();
        assert_eq!(current(&ts).as_deref(), Some("seal"));
        let text = std::fs::read_to_string(ts.join("config.cfg")).unwrap();
        assert_eq!(text, "bind \"w\" \"+forward\"\nmodel \"seal\"\nname \"Player\"\n");
        std::fs::remove_dir_all(&ts).unwrap();
    }

    #[test]
    fn imports_a_model_folder() {
        let root = scratch("import");
        let ts = root.join("ts");
        std::fs::create_dir_all(player_dir(&ts).join("Neo")).unwrap();
        let src = root.join("Downloaded Neo v2");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("neo.mdl"), fake_mdl()).unwrap();
        std::fs::write(src.join("neoT.mdl"), fake_mdl()).unwrap();
        std::fs::write(src.join("neo.bmp"), b"BM").unwrap();
        std::fs::write(src.join("readme.txt"), b"hi").unwrap();
        std::fs::write(src.join("thumbs.db"), b"junk").unwrap();

        let plan = plan_import(&src.join("neo.bmp"), &ts).unwrap();
        assert_eq!(plan.name, "neo");
        assert_eq!(plan.files.len(), 4);
        assert!(plan.exists, "Neo exists (case-insensitively)");
        assert!(import(&plan, &ts, false).is_err());
        let dest = import(&plan, &ts, true).unwrap();
        assert!(dest.ends_with("Neo"), "kept the existing spelling");
        assert!(dest.join("neoT.mdl").is_file() && !dest.join("thumbs.db").exists());

        std::fs::write(src.join("other.mdl"), fake_mdl()).unwrap();
        assert!(plan_import(&src, &ts).unwrap_err().to_string().contains("several"));
        std::fs::remove_file(src.join("other.mdl")).unwrap();
        std::fs::write(src.join("neo.mdl"), b"junk").unwrap();
        assert!(plan_import(&src, &ts).is_err(), "not a model");
        assert!(plan_import(&root.join("ts"), &ts).unwrap_err().to_string().contains("no .mdl"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
