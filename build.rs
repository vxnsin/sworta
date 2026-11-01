use std::{env, fs, path::PathBuf};

fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let cell = size as f32 / 5.0;
    for y in 0..size {
        for x in 0..size {
            let (gx, gy) = (x as f32 / cell, y as f32 / cell);
            let (cx, cy) = (gx.floor() + 0.5, gy.floor() + 0.5);
            if ((gx - cx).powi(2) + (gy - cy).powi(2)).sqrt() < 0.3 {
                let center = gx.floor() == 2.0 && gy.floor() == 2.0;
                let c = if center { [215, 25, 33] } else { [237, 237, 237] };
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
    }
    px
}

fn write_ico(path: &PathBuf, size: u32) {
    let rgba = icon_rgba(size);
    let mut bmp = Vec::new();
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&(size * 2).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0u8; 24]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            bmp.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    bmp.extend(vec![0u8; (size.div_ceil(32) * 4 * size) as usize]);

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    ico.extend_from_slice(&[size as u8, size as u8, 0, 0, 1, 0, 32, 0]);
    ico.extend_from_slice(&(bmp.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend(bmp);
    fs::write(path, ico).unwrap();
}

fn main() {
    let ico = PathBuf::from(env::var("OUT_DIR").unwrap()).join("sworta.ico");
    write_ico(&ico, 64);
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap());
    res.set("ProductName", "SWORTA");
    res.set("FileDescription", "SWORTA - SD card importer");
    res.compile().unwrap();
}
