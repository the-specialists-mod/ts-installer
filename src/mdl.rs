//! GoldSrc studio models (`.mdl`, version 10): enough parsing to pose a model in one frame of a
//! sequence, and a small software rasterizer to draw it with its textures.
//!
//! Layouts follow the HL SDK's `engine/studio.h`; the bone math follows `utils/mdlviewer`
//! (`StudioCalcBoneQuaternion`, `StudioCalcBonePosition`, `StudioSetupBones`). Bone controllers,
//! blending and chrome reflections are left out: a preview at rest doesn't need them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

const IDST: &[u8; 4] = b"IDST";
const STUDIO_X: i32 = 1;
const STUDIO_Y: i32 = 2;
const STUDIO_Z: i32 = 4;
pub const NF_CHROME: i32 = 0x02;
pub const NF_ADDITIVE: i32 = 0x20;
pub const NF_MASKED: i32 = 0x40;

/// Bounds-checked little-endian reads from a file buffer.
struct Bytes<'a> {
    data: &'a [u8],
    what: &'a str,
}

impl<'a> Bytes<'a> {
    fn array<const N: usize>(&self, off: usize) -> Result<[u8; N]> {
        match self.data.get(off..off + N) {
            Some(s) => Ok(s.try_into().expect("length checked")),
            None => bail!("{} is truncated (offset {off})", self.what),
        }
    }
    fn i32(&self, off: usize) -> Result<i32> {
        self.array(off).map(i32::from_le_bytes)
    }
    /// A count or offset: an `i32` that must not be negative.
    fn index(&self, off: usize) -> Result<usize> {
        let v = self.i32(off)?;
        usize::try_from(v).with_context(|| format!("{} has a negative count or offset at {off}", self.what))
    }
    fn f32(&self, off: usize) -> Result<f32> {
        self.array(off).map(f32::from_le_bytes)
    }
    fn u16(&self, off: usize) -> Result<u16> {
        self.array(off).map(u16::from_le_bytes)
    }
    fn i16(&self, off: usize) -> Result<i16> {
        self.array(off).map(i16::from_le_bytes)
    }
    fn vec3(&self, off: usize) -> Result<[f32; 3]> {
        Ok([self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?])
    }
    fn name(&self, off: usize, len: usize) -> Result<String> {
        let s = self.data.get(off..off + len).with_context(|| format!("{} is truncated (offset {off})", self.what))?;
        let end = s.iter().position(|&b| b == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&s[..end]).into_owned())
    }
    fn slice(&self, off: usize, len: usize) -> Result<&'a [u8]> {
        self.data.get(off..off + len).with_context(|| format!("{} is truncated (offset {off}, {len} bytes)", self.what))
    }
}

/// Checks the `IDST` magic and version 10.
pub fn check_header(data: &[u8]) -> Result<()> {
    ensure!(data.len() >= 244 && &data[..4] == IDST, "not a studio model (no IDST header)");
    let version = i32::from_le_bytes(data[4..8].try_into().unwrap());
    ensure!(version == 10, "studio model version {version}, expected 10 (Half-Life)");
    Ok(())
}

struct Bone {
    parent: i32,
    value: [f32; 6],
    scale: [f32; 6],
}

pub struct Sequence {
    pub label: String,
    pub frames: usize,
    motion_type: i32,
    motion_bone: usize,
    anim_index: usize,
    group: usize,
}

/// A skin texture expanded to RGBA.
pub struct Texture {
    pub name: String,
    pub flags: i32,
    pub width: usize,
    pub height: usize,
    /// `width * height` RGBA texels; masked texels (palette index 255) have alpha 0.
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Texture coordinates in texels.
    pub st: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct Triangle {
    pub v: [Vertex; 3],
    /// Index into `Model::textures`, if the skin table points at one.
    pub texture: Option<usize>,
}

/// A studio model posed in one frame: triangles in model space (Z up, facing +X).
pub struct Model {
    pub name: String,
    pub sequences: Vec<Sequence>,
    /// The sequence the triangles are posed in.
    pub sequence: usize,
    pub bones: usize,
    pub textures: Vec<Texture>,
    pub triangles: Vec<Triangle>,
}

/// A 3x4 bone matrix (rotation and translation), as in the SDK.
type Mat = [[f32; 4]; 3];

impl Model {
    /// Loads `path`, with its `<name>T.mdl` textures and `<name>NN.mdl` sequence groups beside
    /// it, posed in frame 0 of its first idle sequence (or sequence 0).
    pub fn load(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&data, |suffix| sibling(path, suffix))
    }

    /// Parses a model; `sibling("T")` or `sibling("01")` reads the companion files.
    pub fn parse(data: &[u8], sibling: impl Fn(&str) -> Result<Vec<u8>>) -> Result<Self> {
        check_header(data)?;
        let b = Bytes { data, what: "model" };
        let name = b.name(8, 64)?;
        let (num_bones, bone_index) = (b.index(140)?, b.index(144)?);
        let (num_seq, seq_index) = (b.index(164)?, b.index(168)?);
        let num_textures = b.index(180)?;
        let num_bodyparts = b.index(204)?;
        let body_index = b.index(208)?;
        ensure!(num_bones <= 512 && num_seq <= 4096 && num_bodyparts <= 64, "model counts are implausible");

        let bones = (0..num_bones)
            .map(|i| {
                let o = bone_index + i * 112;
                let mut value = [0.0; 6];
                let mut scale = [0.0; 6];
                for j in 0..6 {
                    value[j] = b.f32(o + 64 + j * 4)?;
                    scale[j] = b.f32(o + 88 + j * 4)?;
                }
                Ok(Bone { parent: b.i32(o + 32)?, value, scale })
            })
            .collect::<Result<Vec<_>>>()?;

        let sequences = (0..num_seq)
            .map(|i| {
                let o = seq_index + i * 176;
                Ok(Sequence {
                    label: b.name(o, 32)?,
                    frames: b.index(o + 56)?,
                    motion_type: b.i32(o + 68)?,
                    motion_bone: b.index(o + 72)?,
                    anim_index: b.index(o + 124)?,
                    group: b.index(o + 156)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(!sequences.is_empty(), "model has no sequences");
        let sequence = sequences.iter().position(|s| s.label.to_ascii_lowercase().contains("idle")).unwrap_or(0);

        // Textures live in `<name>T.mdl` when the model itself has none.
        let tex_file;
        let tb = if num_textures == 0 {
            tex_file = sibling("T").context("read the texture file (<name>T.mdl)")?;
            check_header(&tex_file)?;
            Bytes { data: &tex_file, what: "texture file" }
        } else {
            Bytes { data, what: "model" }
        };
        let textures = read_textures(&tb)?;
        let skin_ref = tb.index(192)?;
        let skin_index = tb.index(200)?;
        let skins = (0..skin_ref).map(|i| tb.i16(skin_index + i * 2)).collect::<Result<Vec<_>>>()?;

        // Pose the bones.
        let seq = &sequences[sequence];
        let group_file;
        let ab = if seq.group == 0 {
            Bytes { data, what: "model" }
        } else {
            group_file = sibling(&format!("{:02}", seq.group)).context("read the sequence group file")?;
            Bytes { data: &group_file, what: "sequence group" }
        };
        let matrices = pose(&bones, seq, &ab)?;

        // Body 0: the first model of each body part.
        let mut triangles = Vec::new();
        for p in 0..num_bodyparts {
            let o = body_index + p * 76;
            if b.index(o + 64)? == 0 {
                continue;
            }
            let model = b.index(o + 72)?;
            read_model(&b, model, &matrices, &skins, &textures, &mut triangles)?;
        }
        Ok(Self { name, sequences, sequence, bones: bones.len(), textures, triangles })
    }

    /// The axis-aligned bounds of the posed triangles.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for v in self.triangles.iter().flat_map(|t| t.v.iter()) {
            for i in 0..3 {
                lo[i] = lo[i].min(v.pos[i]);
                hi[i] = hi[i].max(v.pos[i]);
            }
        }
        (lo, hi)
    }
}

/// `models/player/x/x.mdl` + "T" = `x/xT.mdl` (also tried lowercase `t`).
fn sibling(path: &Path, suffix: &str) -> Result<Vec<u8>> {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let dir = path.parent().unwrap_or(Path::new("."));
    let candidates: Vec<PathBuf> = [suffix.to_owned(), suffix.to_ascii_lowercase()]
        .iter()
        .flat_map(|s| [format!("{stem}{s}.mdl"), format!("{stem}{s}.MDL")])
        .map(|f| dir.join(f))
        .collect();
    for c in &candidates {
        if let Ok(d) = std::fs::read(c) {
            return Ok(d);
        }
    }
    bail!("{} not found", candidates[0].display())
}

fn read_textures(b: &Bytes) -> Result<Vec<Texture>> {
    let (num, index) = (b.index(180)?, b.index(184)?);
    ensure!(num <= 1024, "too many textures");
    (0..num)
        .map(|i| {
            let o = index + i * 80;
            let (flags, width, height, data) = (b.i32(o + 64)?, b.index(o + 68)?, b.index(o + 72)?, b.index(o + 76)?);
            ensure!(width > 0 && height > 0 && width <= 4096 && height <= 4096, "bad texture size {width}x{height}");
            let pixels = b.slice(data, width * height)?;
            let palette = b.slice(data + width * height, 768)?;
            let masked = flags & NF_MASKED != 0;
            let mut rgba = Vec::with_capacity(width * height * 4);
            for &p in pixels {
                let c = &palette[p as usize * 3..p as usize * 3 + 3];
                rgba.extend_from_slice(&[c[0], c[1], c[2], if masked && p == 255 { 0 } else { 255 }]);
            }
            Ok(Texture { name: b.name(o, 64)?, flags, width, height, rgba })
        })
        .collect()
}

/// One animation channel of a bone at frame 0 (the SDK's run-length `mstudioanimvalue_t` walk).
fn anim_value(b: &Bytes, at: usize, frame: usize) -> Result<i16> {
    let mut at = at;
    let mut k = frame;
    loop {
        let [valid, total] = b.array::<2>(at)?;
        ensure!(total > 0, "bad animation data");
        if (total as usize) > k {
            return if (valid as usize) > k { b.i16(at + (k + 1) * 2) } else { b.i16(at + valid as usize * 2) };
        }
        k -= total as usize;
        at += (valid as usize + 1) * 2;
    }
}

fn pose(bones: &[Bone], seq: &Sequence, b: &Bytes) -> Result<Vec<Mat>> {
    let mut out: Vec<Mat> = Vec::with_capacity(bones.len());
    for (i, bone) in bones.iter().enumerate() {
        let anim = seq.anim_index + i * 12;
        let mut v = [0.0f32; 6];
        for (j, v) in v.iter_mut().enumerate() {
            let off = b.u16(anim + j * 2)? as usize;
            *v = bone.value[j];
            if off != 0 {
                *v += anim_value(b, anim + off, 0)? as f32 * bone.scale[j];
            }
        }
        let mut pos = [v[0], v[1], v[2]];
        if i == seq.motion_bone {
            for (bit, axis) in [(STUDIO_X, 0), (STUDIO_Y, 1), (STUDIO_Z, 2)] {
                if seq.motion_type & bit != 0 {
                    pos[axis] = 0.0;
                }
            }
        }
        let local = matrix(quaternion([v[3], v[4], v[5]]), pos);
        let m = match usize::try_from(bone.parent) {
            Ok(p) if p < i => concat(&out[p], &local),
            Ok(_) => bail!("bone {i} has a bad parent {}", bone.parent),
            Err(_) => local,
        };
        out.push(m);
    }
    Ok(out)
}

/// The SDK's `AngleQuaternion` (angles in radians: roll, pitch, yaw about X, Y, Z).
fn quaternion(a: [f32; 3]) -> [f32; 4] {
    let (sy, cy) = (a[2] * 0.5).sin_cos();
    let (sp, cp) = (a[1] * 0.5).sin_cos();
    let (sr, cr) = (a[0] * 0.5).sin_cos();
    [sr * cp * cy - cr * sp * sy, cr * sp * cy + sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy]
}

/// The SDK's `QuaternionMatrix` plus a translation column.
fn matrix(q: [f32; 4], pos: [f32; 3]) -> Mat {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * y * y - 2.0 * z * z, 2.0 * x * y - 2.0 * w * z, 2.0 * x * z + 2.0 * w * y, pos[0]],
        [2.0 * x * y + 2.0 * w * z, 1.0 - 2.0 * x * x - 2.0 * z * z, 2.0 * y * z - 2.0 * w * x, pos[1]],
        [2.0 * x * z - 2.0 * w * y, 2.0 * y * z + 2.0 * w * x, 1.0 - 2.0 * x * x - 2.0 * y * y, pos[2]],
    ]
}

/// The SDK's `R_ConcatTransforms`: `a * b`.
fn concat(a: &Mat, b: &Mat) -> Mat {
    let mut o = [[0.0; 4]; 3];
    for i in 0..3 {
        for j in 0..4 {
            o[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
        o[i][3] += a[i][3];
    }
    o
}

fn transform(m: &Mat, v: [f32; 3]) -> [f32; 3] {
    let r = rotate(m, v);
    [r[0] + m[0][3], r[1] + m[1][3], r[2] + m[2][3]]
}

fn rotate(m: &Mat, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn read_model(b: &Bytes, o: usize, bones: &[Mat], skins: &[i16], textures: &[Texture], out: &mut Vec<Triangle>) -> Result<()> {
    let (num_mesh, mesh_index) = (b.index(o + 72)?, b.index(o + 76)?);
    let (num_verts, vert_info, vert_index) = (b.index(o + 80)?, b.index(o + 84)?, b.index(o + 88)?);
    let (num_norms, norm_info, norm_index) = (b.index(o + 92)?, b.index(o + 96)?, b.index(o + 100)?);
    ensure!(num_verts <= 65536 && num_norms <= 65536 && num_mesh <= 4096, "model counts are implausible");
    let bone = |info: usize, i: usize| -> Result<&Mat> {
        let n = b.slice(info + i, 1)?[0] as usize;
        bones.get(n).with_context(|| format!("vertex bound to missing bone {n}"))
    };
    let verts = (0..num_verts).map(|i| Ok(transform(bone(vert_info, i)?, b.vec3(vert_index + i * 12)?))).collect::<Result<Vec<_>>>()?;
    let norms =
        (0..num_norms).map(|i| Ok(normalize(rotate(bone(norm_info, i)?, b.vec3(norm_index + i * 12)?)))).collect::<Result<Vec<_>>>()?;

    for m in 0..num_mesh {
        let mo = mesh_index + m * 20;
        let (tri_index, skin) = (b.index(mo + 4)?, b.index(mo + 8)?);
        let texture = skins.get(skin).and_then(|&t| usize::try_from(t).ok()).filter(|&t| t < textures.len());
        let mut at = tri_index;
        loop {
            let n = b.i16(at)? as i32;
            at += 2;
            if n == 0 {
                break;
            }
            let fan = n < 0;
            let count = n.unsigned_abs() as usize;
            let mut strip = Vec::with_capacity(count);
            for _ in 0..count {
                let (vi, ni) = (b.i16(at)? as usize, b.i16(at + 2)? as usize);
                let st = [b.i16(at + 4)? as f32, b.i16(at + 6)? as f32];
                at += 8;
                let pos = *verts.get(vi).context("triangle uses a missing vertex")?;
                let normal = *norms.get(ni).context("triangle uses a missing normal")?;
                strip.push(Vertex { pos, normal, st });
            }
            for k in 2..count {
                let v = if fan {
                    [strip[0], strip[k - 1], strip[k]]
                } else if k % 2 == 0 {
                    [strip[k - 2], strip[k - 1], strip[k]]
                } else {
                    [strip[k - 1], strip[k - 2], strip[k]]
                };
                out.push(Triangle { v, texture });
            }
        }
    }
    Ok(())
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { v.map(|c| c / l) } else { v }
}

/// Draws `model` turned `yaw` radians about its vertical axis, seen from the front, into a
/// `width` x `height` RGBA buffer (transparent background). `ssaa` > 1 renders that many times
/// larger and averages down, for smoother edges.
pub fn render(model: &Model, yaw: f32, width: usize, height: usize, ssaa: usize) -> Vec<u8> {
    let ssaa = ssaa.max(1);
    let (w, h) = (width * ssaa, height * ssaa);
    let mut color = vec![0u8; w * h * 4];
    let mut depth = vec![f32::MIN; w * h];

    // Fit the model so it stays the same size at every angle.
    let (lo, hi) = model.bounds();
    if lo[0] > hi[0] {
        return vec![0; width * height * 4];
    }
    let center = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let radius = model
        .triangles
        .iter()
        .flat_map(|t| t.v.iter())
        .map(|v| ((v.pos[0] - center[0]).powi(2) + (v.pos[1] - center[1]).powi(2)).sqrt())
        .fold(1.0f32, f32::max);
    let scale = (w as f32 * 0.94 / (2.0 * radius)).min(h as f32 * 0.94 / (hi[2] - lo[2]).max(1.0));
    let (sin, cos) = yaw.sin_cos();
    // The camera looks down -X: screen right is +Y, up is +Z, and larger X is nearer.
    let project = |p: [f32; 3]| {
        let (x, y) = (p[0] - center[0], p[1] - center[1]);
        let (rx, ry) = (x * cos - y * sin, x * sin + y * cos);
        [w as f32 / 2.0 + ry * scale, h as f32 / 2.0 - (p[2] - center[2]) * scale, rx]
    };
    let light = normalize([0.6, 0.45, 0.65]);
    let shade = |n: [f32; 3]| {
        let n = [n[0] * cos - n[1] * sin, n[0] * sin + n[1] * cos, n[2]];
        let d = n[0] * light[0] + n[1] * light[1] + n[2] * light[2];
        // Half-Lambert, as in later Source shading: soft, with no black backs.
        let l = d * 0.5 + 0.5;
        0.35 + 0.85 * l * l
    };

    for tri in &model.triangles {
        let tex = tri.texture.map(|t| &model.textures[t]);
        let p = tri.v.map(|v| project(v.pos));
        let lit = tri.v.map(|v| shade(v.normal));
        let st = if tex.is_some_and(|t| t.flags & NF_CHROME != 0) {
            // A stand-in for chrome: map the view-space normal onto the texture.
            let t = tex.unwrap();
            tri.v.map(|v| {
                let n = [v.normal[0] * sin + v.normal[1] * cos, v.normal[2]];
                [(n[0] * 0.5 + 0.5) * t.width as f32, (0.5 - n[1] * 0.5) * t.height as f32]
            })
        } else {
            tri.v.map(|v| v.st)
        };
        let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let x1 = (p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil().min(w as f32 - 1.0)).max(0.0) as usize;
        let y0 = p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
        let y1 = (p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil().min(h as f32 - 1.0)).max(0.0) as usize;
        for y in y0..=y1 {
            let py = y as f32 + 0.5;
            for x in x0..=x1 {
                let px = x as f32 + 0.5;
                let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
                let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                let i = y * w + x;
                if z <= depth[i] {
                    continue;
                }
                let l = w0 * lit[0] + w1 * lit[1] + w2 * lit[2];
                let rgb = match tex {
                    Some(t) => {
                        let s = (w0 * st[0][0] + w1 * st[1][0] + w2 * st[2][0]).floor() as i64;
                        let u = (w0 * st[0][1] + w1 * st[1][1] + w2 * st[2][1]).floor() as i64;
                        let ti = (u.rem_euclid(t.height as i64) as usize * t.width + s.rem_euclid(t.width as i64) as usize) * 4;
                        if t.rgba[ti + 3] == 0 {
                            continue;
                        }
                        [t.rgba[ti], t.rgba[ti + 1], t.rgba[ti + 2]]
                    }
                    None => [180, 180, 180],
                };
                let additive = tex.is_some_and(|t| t.flags & NF_ADDITIVE != 0);
                let o = i * 4;
                for c in 0..3 {
                    let v = rgb[c] as f32 * l;
                    color[o + c] = if additive { (color[o + c] as f32 + v).min(255.0) } else { v.min(255.0) } as u8;
                }
                color[o + 3] = 255;
                if !additive {
                    depth[i] = z;
                }
            }
        }
    }
    if ssaa == 1 {
        return color;
    }
    downsample(&color, w, width, height, ssaa)
}

/// Averages `n` x `n` blocks (alpha-weighted, so edges don't darken).
fn downsample(src: &[u8], src_w: usize, width: usize, height: usize, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0u32; 4];
            for dy in 0..n {
                for dx in 0..n {
                    let o = ((y * n + dy) * src_w + x * n + dx) * 4;
                    let a = src[o + 3] as u32;
                    for c in 0..3 {
                        acc[c] += src[o + c] as u32 * a;
                    }
                    acc[3] += a;
                }
            }
            let o = (y * width + x) * 4;
            for c in 0..3 {
                out[o + c] = acc[c].checked_div(acc[3]).unwrap_or(0) as u8;
            }
            out[o + 3] = (acc[3] / (n * n) as u32) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> Vec<PathBuf> {
        let dir = Path::new(&std::env::var_os("HOME").unwrap_or_default()).join("dev/hl/ts_original/models/player");
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut out: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| {
                let n = e.file_name();
                e.path().join(format!("{}.mdl", n.to_string_lossy()))
            })
            .filter(|p| p.is_file())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn parses_shipped_models() {
        let models = shipped();
        if models.is_empty() {
            eprintln!("TS 3.0 models not found; skipping");
            return;
        }
        assert_eq!(models.len(), 7);
        for path in models {
            let m = Model::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            assert_eq!(m.sequences.len(), 171, "{}", path.display());
            assert_eq!(m.sequences[m.sequence].label, "standard_idle");
            assert_eq!(m.textures.len(), 2);
            assert!(m.triangles.len() > 500, "{}: {} triangles", path.display(), m.triangles.len());
            assert!(m.triangles.iter().all(|t| t.texture.is_some()));
            // Standing: about 72 units tall (player hull), feet near the origin's floor, facing +X.
            let (lo, hi) = m.bounds();
            let tall = hi[2] - lo[2];
            assert!((55.0..90.0).contains(&tall), "{}: height {tall}", path.display());
            assert!(hi[1] - lo[1] < tall && hi[0] - lo[0] < tall, "{}: wider than tall: {lo:?} {hi:?}", path.display());
            let img = render(&m, 0.0, 80, 100, 1);
            let covered = img.chunks(4).filter(|p| p[3] == 255).count();
            assert!(covered > 800, "{}: only {covered} pixels drawn", path.display());
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Model::parse(b"not a model", |_| bail!("none")).is_err());
        let mut data = vec![0u8; 300];
        data[..4].copy_from_slice(IDST);
        data[4] = 10;
        // Zero counts everywhere: no sequences.
        assert!(Model::parse(&data, |_| bail!("none")).is_err());
        data[4] = 44;
        assert!(check_header(&data).unwrap_err().to_string().contains("version 44"));
    }

    #[test]
    fn anim_values_follow_runs() {
        // Run 1: 2 stored values covering 3 frames (frame 2 repeats the last); run 2: 1 value for 2 frames.
        let raw: Vec<u8> = [[2u8, 3], 10i16.to_le_bytes(), 20i16.to_le_bytes(), [1, 2], 30i16.to_le_bytes()].concat();
        let b = Bytes { data: &raw, what: "test" };
        let got: Vec<i16> = (0..5).map(|f| anim_value(&b, 0, f).unwrap()).collect();
        assert_eq!(got, [10, 20, 20, 30, 30]);
    }
}
