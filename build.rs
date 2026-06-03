use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const SVG: &str = "assets/icon.svg";
const ICONS_DIR: &str = "assets/icons";
const PNG_SIZES: [u32; 8] = [16, 32, 48, 64, 128, 256, 512, 1024];
const ICO_SIZES: [u32; 4] = [16, 32, 48, 256];

fn main() {
    println!("cargo:rerun-if-changed={SVG}");
    println!("cargo:rerun-if-changed=build.rs");

    std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(build_assets)
        .expect("spawn build thread")
        .join()
        .expect("join build thread");
}

fn build_assets() {
    fs::create_dir_all(ICONS_DIR).expect("create icons dir");
    let svg = Path::new(SVG);

    let mut icons_updated = false;
    for size in PNG_SIZES {
        let png = icon_png_path(size);
        if is_stale(svg, &png) {
            render_png(SVG, &png, size);
            icons_updated = true;
        }
    }

    let ico_path = Path::new(ICONS_DIR).join("icon.ico");
    let ico_pngs = icon_png_paths(&ICO_SIZES);
    let ico_updated = icons_updated || is_any_newer_than(&ico_pngs, &ico_path);
    if ico_updated {
        write_ico(&ico_pngs, ico_path);
    }

    let icns_path = Path::new(ICONS_DIR).join("icon.icns");
    if icons_updated || is_any_newer_than(&icon_png_paths(&PNG_SIZES), &icns_path) {
        write_icns(Path::new(ICONS_DIR));
    }

    #[cfg(windows)]
    if ico_updated {
        let mut res = winres::WindowsResource::new();
        res.set_icon(&format!("{ICONS_DIR}/icon.ico"));
        res.compile().expect("embed Windows icon");
    }

    // Slint codegen is stack-heavy on Windows.
    slint_build::compile("ui/app.slint").expect("compile slint");
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn is_stale(source: &Path, dest: &Path) -> bool {
    match modified(dest) {
        None => true,
        Some(dest_mtime) => modified(source).is_some_and(|source_mtime| source_mtime > dest_mtime),
    }
}

fn is_any_newer_than(sources: &[PathBuf], dest: &Path) -> bool {
    let Some(dest_mtime) = modified(dest) else {
        return true;
    };
    sources
        .iter()
        .any(|source| modified(source).is_some_and(|source_mtime| source_mtime > dest_mtime))
}

fn icon_png_path(size: u32) -> PathBuf {
    Path::new(ICONS_DIR).join(format!("icon-{size}.png"))
}

fn icon_png_paths(sizes: &[u32]) -> Vec<PathBuf> {
    sizes.iter().map(|size| icon_png_path(*size)).collect()
}

fn render_png(svg_path: &str, png_path: &Path, size: u32) {
    let svg_data = fs::read(svg_path).unwrap_or_else(|err| panic!("read {svg_path}: {err}"));
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(&svg_data, &opt).unwrap_or_else(|err| panic!("parse svg: {err}"));
    let mut pixmap = tiny_skia::Pixmap::new(size, size).unwrap_or_else(|| panic!("pixmap {size}"));
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .save_png(png_path)
        .unwrap_or_else(|err| panic!("write {}: {err}", png_path.display()));
}

fn write_ico(png_paths: &[PathBuf], ico_path: PathBuf) {
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for path in png_paths {
        let file = fs::File::open(path)
            .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
        let image = ico::IconImage::read_png(file)
            .unwrap_or_else(|err| panic!("read png {}: {err}", path.display()));
        let entry = ico::IconDirEntry::encode(&image)
            .unwrap_or_else(|err| panic!("encode {}: {err}", path.display()));
        icon_dir.add_entry(entry);
    }
    let file = fs::File::create(&ico_path)
        .unwrap_or_else(|err| panic!("create {}: {err}", ico_path.display()));
    icon_dir
        .write(file)
        .unwrap_or_else(|err| panic!("write {}: {err}", ico_path.display()));
}

fn write_icns(icons_dir: &Path) {
    let iconset = icons_dir.join("icon.iconset");
    let _ = fs::remove_dir_all(&iconset);
    fs::create_dir_all(&iconset).expect("create iconset");

    let mappings: [(&str, u32); 10] = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ];
    for (name, size) in mappings {
        fs::copy(icon_png_path(size), iconset.join(name))
            .unwrap_or_else(|err| panic!("copy {name}: {err}"));
    }

    let icns_path = icons_dir.join("icon.icns");
    let status = Command::new("iconutil")
        .args(["-c", "icns", "-o"])
        .arg(&icns_path)
        .arg(&iconset)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => eprintln!(
            "cargo:warning=iconutil failed ({s}); macOS .icns not generated (Dock may still use PNG window icon)"
        ),
        Err(err) => eprintln!("cargo:warning=iconutil not available: {err}"),
    }
    let _ = fs::remove_dir_all(&iconset);
}
