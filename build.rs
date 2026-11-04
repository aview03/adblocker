use std::fs;

const RED: [u8; 3] = [0xEC, 0x1B, 0x1B];
const SIZES: [u32; 5] = [16, 32, 48, 64, 256];

/// Rounded red square with a white ring, 4x4 supersampled. Returns BGRA, top-down.
fn render(size: u32) -> Vec<u8> {
    let s = size as f32;
    let corner = s * 0.14;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut a, mut ring) = (0.0f32, 0.0f32);
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let dx = (fx - s / 2.0).abs() - (s / 2.0 - corner);
                    let dy = (fy - s / 2.0).abs() - (s / 2.0 - corner);
                    if dx.max(0.0).hypot(dy.max(0.0)) <= corner {
                        a += 1.0;
                        let d = (fx - s / 2.0).hypot(fy - s / 2.0);
                        if d < s * 0.30 && d > s * 0.18 {
                            ring += 1.0;
                        }
                    }
                }
            }
            let alpha = (a / 16.0 * 255.0).round() as u8;
            let w = if a > 0.0 { ring / a } else { 0.0 };
            for c in (0..3).rev() {
                out.push((RED[c] as f32 * (1.0 - w) + 255.0 * w).round() as u8);
            }
            out.push(alpha);
        }
    }
    out
}

fn dib(size: u32) -> Vec<u8> {
    let px = render(size);
    let mut d = Vec::new();
    d.extend_from_slice(&40u32.to_le_bytes());
    d.extend_from_slice(&size.to_le_bytes());
    d.extend_from_slice(&(size * 2).to_le_bytes());
    d.extend_from_slice(&1u16.to_le_bytes());
    d.extend_from_slice(&32u16.to_le_bytes());
    d.extend_from_slice(&[0u8; 24]);
    for row in (0..size).rev() {
        let i = (row * size * 4) as usize;
        d.extend_from_slice(&px[i..i + (size * 4) as usize]);
    }
    let mask_row = ((size + 31) / 32 * 4) as usize;
    d.extend(std::iter::repeat(0u8).take(mask_row * size as usize));
    d
}

fn ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| dib(s)).collect();
    let mut f = Vec::new();
    f.extend_from_slice(&[0, 0, 1, 0]);
    f.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (&s, img) in SIZES.iter().zip(&images) {
        let b = if s >= 256 { 0 } else { s as u8 };
        f.extend_from_slice(&[b, b, 0, 0]);
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&32u16.to_le_bytes());
        f.extend_from_slice(&(img.len() as u32).to_le_bytes());
        f.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        f.extend(img);
    }
    f
}

fn main() {
    // RC.EXE chokes on non-ASCII absolute paths, so keep everything relative to the crate root.
    fs::write("app.ico", ico()).unwrap();
    fs::write("app.rc", "1 ICON \"app.ico\"
1 24 \"app.manifest\"
").unwrap();

    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().unwrap();
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=build.rs");
}
