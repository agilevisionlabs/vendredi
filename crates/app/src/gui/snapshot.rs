//! Test-only rendering of egui output to a PNG, so pages can be looked at
//! where screenshots are not available. Triangles are filled in software
//! with the font atlas; the result matches the GPU closely enough to judge
//! layout, colour and icons.
use super::*;
use egui::{Color32, TextureId, epaint::Primitive};
use std::collections::HashMap;

pub(super) struct Canvas {
    textures: HashMap<TextureId, (usize, usize, Vec<Color32>)>,
}

impl Canvas {
    pub(super) fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    /// Renders one frame's output at `scale` pixels per point.
    pub(super) fn render(
        &mut self,
        ctx: &egui::Context,
        output: egui::FullOutput,
        size: egui::Vec2,
        scale: f32,
    ) -> (usize, usize, Vec<u8>) {
        for (id, delta) in output.textures_delta.set {
            let egui::ImageData::Color(image) = &delta.image;
            let [w, h] = image.size;
            match delta.pos {
                None => {
                    self.textures.insert(id, (w, h, image.pixels.clone()));
                }
                Some([x, y]) => {
                    let (tw, _, pixels) = self.textures.get_mut(&id).expect("texture");
                    for row in 0..h {
                        for col in 0..w {
                            pixels[(y + row) * *tw + x + col] = image.pixels[row * w + col];
                        }
                    }
                }
            }
        }
        // The font atlas as it is now, in case earlier frames updated it
        // without passing through this canvas.
        let atlas = ctx.fonts(|f| f.image());
        self.textures.insert(
            TextureId::default(),
            (atlas.size[0], atlas.size[1], atlas.pixels.clone()),
        );
        let (width, height) = ((size.x * scale) as usize, (size.y * scale) as usize);
        let mut out = vec![[0f32; 4]; width * height];
        for clipped in ctx.tessellate(output.shapes, scale) {
            let Primitive::Mesh(mesh) = clipped.primitive else {
                continue;
            };
            let Some((tw, th, tex)) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = clipped.clip_rect;
            let (cx0, cy0) = (
                (clip.min.x * scale).max(0.) as usize,
                (clip.min.y * scale).max(0.) as usize,
            );
            let (cx1, cy1) = (
                ((clip.max.x * scale) as usize).min(width),
                ((clip.max.y * scale) as usize).min(height),
            );
            for t in mesh.indices.chunks_exact(3) {
                let v = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize]);
                let p = v.map(|v| (v.pos.x * scale, v.pos.y * scale));
                let area =
                    (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = p
                    .iter()
                    .map(|q| q.0)
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(cx0 as f32) as usize;
                let x1 = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(cx1);
                let y0 = p
                    .iter()
                    .map(|q| q.1)
                    .fold(f32::MAX, f32::min)
                    .floor()
                    .max(cy0 as f32) as usize;
                let y1 = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(cy1);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 =
                            ((p[1].0 - px) * (p[2].1 - py) - (p[2].0 - px) * (p[1].1 - py)) / area;
                        let w1 =
                            ((p[2].0 - px) * (p[0].1 - py) - (p[0].0 - px) * (p[2].1 - py)) / area;
                        let w2 = 1. - w0 - w1;
                        if w0 < 0. || w1 < 0. || w2 < 0. {
                            continue;
                        }
                        let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        let texel = tex[((vv * *th as f32) as usize).min(th - 1) * tw
                            + ((u * *tw as f32) as usize).min(tw - 1)];
                        let mut src = [0f32; 4];
                        for (c, s) in src.iter_mut().enumerate() {
                            let vc = w0 * f32::from(v[0].color[c])
                                + w1 * f32::from(v[1].color[c])
                                + w2 * f32::from(v[2].color[c]);
                            *s = vc / 255. * f32::from(texel[c]) / 255.;
                        }
                        // Premultiplied alpha over what is already there.
                        let dst = &mut out[y * width + x];
                        for c in 0..4 {
                            dst[c] = src[c] + dst[c] * (1. - src[3]);
                        }
                    }
                }
            }
        }
        let bytes = out
            .iter()
            .flat_map(|p| {
                let a = p[3].max(1e-6);
                [p[0] / a, p[1] / a, p[2] / a, p[3]].map(|c| (c.clamp(0., 1.) * 255.).round() as u8)
            })
            .collect();
        (width, height, bytes)
    }
}

/// A PNG of RGBA pixels, with stored (uncompressed) deflate blocks.
pub(super) fn png(width: usize, height: usize, rgba: &[u8]) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for b in bytes {
            c ^= u32::from(*b);
            for _ in 0..8 {
                c = if c & 1 == 1 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(data);
        out.extend(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width * 4 + 1) * height);
    for row in rgba.chunks(width * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<_> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = block.len() as u16;
        z.extend(len.to_le_bytes());
        z.extend((!len).to_le_bytes());
        z.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + u32::from(*x)) % 65521;
        b = (b + a) % 65521;
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::new();
    header.extend((width as u32).to_be_bytes());
    header.extend((height as u32).to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::tests::{click, frame, wait_for};

    /// Renders one frame of the desktop to a PNG in `dir`.
    fn shoot(
        dir: &Path,
        name: &str,
        ctx: &egui::Context,
        size: egui::Vec2,
        draw: impl FnMut(&egui::Context),
    ) {
        // Let layout settle and fades finish: a second of frames, with the
        // clock moving, then the frame that is drawn.
        let mut draw = draw;
        let start = ctx.input(|i| i.time);
        let input = |n: usize| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(start + n as f64 * 0.1),
            ..Default::default()
        };
        for n in 1..10 {
            let _ = ctx.run(input(n), &mut draw);
        }
        let output = ctx.run(input(10), &mut draw);
        let mut canvas = Canvas::new();
        let (w, h, rgba) = canvas.render(ctx, output, size, 1.);
        fs::write(dir.join(format!("{name}.png")), png(w, h, &rgba)).unwrap();
    }

    /// Writes pictures of the main pages, light and dark, to the folder in
    /// `VENDREDI_SNAPSHOTS`, to look at where screenshots are not possible.
    #[test]
    #[ignore = "writes pictures; set VENDREDI_SNAPSHOTS"]
    fn pictures_of_the_main_pages() -> anyhow::Result<()> {
        let Some(out) = std::env::var_os("VENDREDI_SNAPSHOTS").map(PathBuf::from) else {
            return Ok(());
        };
        fs::create_dir_all(&out)?;
        let size = egui::vec2(1280., 800.);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let tag = if theme == egui::Theme::Dark {
                "dark"
            } else {
                "light"
            };
            let ctx = egui::Context::default();
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("test screen size"), size));
            let mut welcome = super::super::session::Session::blank();
            install(&ctx);
            ctx.set_theme(theme);
            shoot(&out, &format!("welcome-{tag}"), &ctx, size, |c| {
                welcome.show(c)
            });
            let dir = tempfile::tempdir()?;
            crate::sample(dir.path())?;
            let file = dir.path().join("vendredi.toml");
            fs::write(&file, fs::read_to_string(&file)?.replace(":8180", ":0"))?;
            let mut app = Desktop::open(dir.path().into(), &ctx)?;
            ctx.set_theme(theme);
            shoot(&out, &format!("empty-{tag}"), &ctx, size, |c| app.show(c));
            click(&mut app, &ctx, "Send sample order")?;
            wait_for(&mut app, "replies", |a| {
                a.transfers
                    .values()
                    .filter(|t| t.status == "Delivered")
                    .count()
                    == 2
            })?;
            let reply = app
                .transfers
                .values()
                .find(|t| t.kind == "ORDRSP")
                .unwrap()
                .id;
            app.select(reply);
            wait_for(&mut app, "inspection", |a| a.detail["transfer"] == reply)?;
            frame(&mut app, &ctx, vec![]);
            shoot(&out, &format!("traffic-{tag}"), &ctx, size, |c| app.show(c));
            click(&mut app, &ctx, "Cases")?;
            shoot(&out, &format!("cases-{tag}"), &ctx, size, |c| app.show(c));
            click(&mut app, &ctx, &with_icon(icon::PLUS, "Add partner"))?;
            shoot(&out, &format!("partner-setup-{tag}"), &ctx, size, |c| {
                app.show(c)
            });
            app.partner_form = None;
            app.edit_script_link(
                Path::new("cases/confirm-orders/confirm.js"),
                "Supplier",
                None,
            );
            shoot(&out, &format!("script-{tag}"), &ctx, size, |c| app.show(c));
        }
        Ok(())
    }
}
