use image::{ImageBuffer, Rgba};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    let crate_name = env::var("CARGO_BIN_NAME").unwrap_or_default();
    let building_installer = crate_name.is_empty() || crate_name == "sinorag-installer";

    // Only the installer (not the uninstaller) embeds the compressed payload.
    if building_installer {
        let payload_path = manifest_dir.join("sinorag.7z");
        if !payload_path.exists() {
            panic!(
                "missing compressed SinoRAG payload at {}",
                payload_path.display()
            );
        }
        println!(
            "cargo:rustc-env=SINORAG_PAYLOAD_PATH={}",
            payload_path.display()
        );
        println!("cargo:rerun-if-changed={}", payload_path.display());
    }

    // Bake in the SinoRAG version this installer ships, read from the workspace
    // crate manifest so it tracks the bundled payload without a manual bump.
    let workspace_cargo = manifest_dir.join("../../Cargo.toml");
    let version = read_package_version(&workspace_cargo);
    println!("cargo:rustc-env=EXPECTED_SINORAG_VERSION={version}");
    println!("cargo:rerun-if-changed={}", workspace_cargo.display());

    let png_path = out_dir.join("sinorag-icon.png");
    let ico_path = out_dir.join("SinoRAG.ico");
    write_icon_png(&png_path);
    write_icon_ico(&png_path, &ico_path);
    println!("cargo:rustc-env=SINORAG_ICON_PNG={}", png_path.display());
    println!("cargo:rustc-env=SINORAG_ICON_ICO={}", ico_path.display());
}

/// Read the `version` from the `[package]` table of a Cargo manifest.
fn read_package_version(path: &Path) -> String {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("version") {
                if let Some((_, value)) = rest.split_once('=') {
                    return value.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    panic!("no [package] version found in {}", path.display());
}

fn write_icon_png(path: &Path) {
    let mut img = ImageBuffer::from_pixel(128, 128, Rgba([18, 50, 53, 255]));
    for y in 0..128 {
        for x in 0..128 {
            let tint = ((x + y) / 14) as u8;
            img.put_pixel(
                x,
                y,
                Rgba([18 + tint.min(16), 50 + tint.min(22), 53 + tint.min(20), 255]),
            );
        }
    }
    for y in 103..112 {
        for x in 13..115 {
            img.put_pixel(x, y, Rgba([154, 73, 55, 255]));
        }
    }
    for y in 113..118 {
        for x in 13..83 {
            img.put_pixel(x, y, Rgba([197, 154, 61, 255]));
        }
    }
    draw_text(&mut img, 12, 32, "SINO", 4, Rgba([247, 244, 234, 255]));
    draw_text(&mut img, 22, 73, "RAG", 4, Rgba([247, 244, 234, 255]));
    img.save(path).expect("write png icon");
}

fn write_icon_ico(png_path: &Path, ico_path: &Path) {
    let image = ico::IconImage::read_png(fs::File::open(png_path).expect("open png"))
        .expect("read png for ico");
    let dir_entry = ico::IconDirEntry::encode(&image).expect("encode ico entry");
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    dir.add_entry(dir_entry);
    dir.write(fs::File::create(ico_path).expect("create ico"))
        .expect("write ico");
}

fn draw_text(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    text: &str,
    scale: u32,
    color: Rgba<u8>,
) {
    let mut cursor = x;
    for ch in text.chars() {
        if ch == ' ' {
            cursor += 4 * scale;
            continue;
        }
        draw_char(img, cursor, y, ch, scale, color);
        cursor += 6 * scale;
    }
}

fn draw_char(
    img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    ch: char,
    scale: u32,
    color: Rgba<u8>,
) {
    let rows = match ch {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'G' => [
            "01111", "10000", "10000", "10111", "10001", "10001", "01111",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        _ => [
            "00000", "00000", "00000", "00000", "00000", "00000", "00000",
        ],
    };

    for (row_idx, row) in rows.iter().enumerate() {
        for (col_idx, bit) in row.as_bytes().iter().enumerate() {
            if *bit == b'1' {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = x + col_idx as u32 * scale + dx;
                        let py = y + row_idx as u32 * scale + dy;
                        if px < img.width() && py < img.height() {
                            img.put_pixel(px, py, color);
                        }
                    }
                }
            }
        }
    }
}
