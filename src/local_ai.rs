//! Local background removal and upscaling through one small vision.cpp runtime.
//!
//! First use downloads the pinned engine and the selected model. Source images are
//! decoded to a scratch PNG, inference runs as a local process, and only the checked
//! result is copied into the audit's `optimized/` tree.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
use flate2::read::GzDecoder;
use image::ImageDecoder as _;
use sha2::{Digest, Sha256};

use crate::{convert, scan};

const RUNTIME_DIR: &str = "visioncpp-0.3.0";
const MAX_UPSCALED_PIXELS: u64 = 40_000_000;
const ESRGAN_TILE: &str = "256";
static SCRATCH_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    RemoveBackground,
    Upscale,
}

impl Tool {
    fn command(self) -> &'static str {
        match self {
            Self::RemoveBackground => "birefnet",
            Self::Upscale => "esrgan",
        }
    }

    fn output_suffix(self) -> &'static str {
        match self {
            Self::RemoveBackground => "-background-removed",
            Self::Upscale => "-4x",
        }
    }

    fn model(self) -> Asset {
        match self {
            Self::RemoveBackground => BIREFNET,
            Self::Upscale => ESRGAN,
        }
    }
}

#[derive(Clone, Copy)]
struct Asset {
    filename: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const RUNTIME: Asset = Asset {
    filename: "visioncpp-linux-x64-0.3.0.tar.gz",
    url: "https://github.com/Acly/vision.cpp/releases/download/v0.3.0/visioncpp-linux-x64-0.3.0.tar.gz",
    sha256: "75d2fd873189202173681d7c9da6ef1b358e445274744793f153c161b09c026a",
    bytes: 15_560_471,
};
const BIREFNET: Asset = Asset {
    filename: "BiRefNet-lite-F16.gguf",
    url: "https://huggingface.co/Acly/BiRefNet-GGUF/resolve/4f6018e2f35cedf26c8ddf0fec1475252f8ba280/BiRefNet-lite-F16.gguf?download=true",
    sha256: "7b5397a2c98d66677f8f74317774bbeac49dbb321b8a3dc744af913db71d4fa5",
    bytes: 88_647_936,
};
const ESRGAN: Asset = Asset {
    filename: "ESRGAN-4x-foolhardy_Remacri-F16.gguf",
    url: "https://huggingface.co/Acly/Real-ESRGAN-GGUF/resolve/eba228c479e796c97bc17cb98c48c245144f67f3/ESRGAN-4x-foolhardy_Remacri-F16.gguf?download=true",
    sha256: "843aa7c4bcf5919b7f5b72eef8d8cfd9df9949c1837002e3f6d9bf07c1b3af5a",
    bytes: 33_451_392,
};

pub struct Prepared {
    runtime: PathBuf,
    model: PathBuf,
    tool: Tool,
}

pub fn available() -> bool {
    cfg!(all(target_os = "linux", target_arch = "x86_64"))
        || data_dir()
            .ok()
            .and_then(|base| discover_runtime(&base).ok().flatten())
            .is_some()
}

pub fn installed(tool: Tool) -> bool {
    let Ok(base) = data_dir() else {
        return false;
    };
    discover_runtime(&base).ok().flatten().is_some()
        && std::fs::metadata(base.join("models").join(tool.model().filename))
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() == tool.model().bytes)
}

pub fn prepare(tool: Tool, cancelled: &AtomicBool) -> Result<Prepared, String> {
    check_cancelled(cancelled)?;
    let base = data_dir()?;
    std::fs::create_dir_all(&base).map_err(|error| format!("AI storage: {error}"))?;
    let runtime = ensure_runtime(&base, cancelled)?;
    let model = ensure_asset(
        &base.join("models").join(tool.model().filename),
        tool.model(),
        cancelled,
    )?;
    Ok(Prepared {
        runtime,
        model,
        tool,
    })
}

pub fn process(
    prepared: Prepared,
    root: &Path,
    out_dir: &Path,
    source: &Path,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    check_cancelled(cancelled)?;
    let (decoded, _profile) = scan::decode_for_conversion(source).map_err(|error| match error {
        scan::ConversionDecodeError::AnimatedGif => "animated GIFs cannot use local AI".to_string(),
        scan::ConversionDecodeError::AnimatedPng => {
            "animated PNG files cannot use local AI".to_string()
        }
        scan::ConversionDecodeError::AnimatedWebP => {
            "animated WebP files cannot use local AI".to_string()
        }
        scan::ConversionDecodeError::AnimatedJpegXl => {
            "animated JPEG XL files cannot use local AI".to_string()
        }
        scan::ConversionDecodeError::Failed => "the source image would not decode".to_string(),
    })?;
    let (width, height) = (decoded.width(), decoded.height());
    if prepared.tool == Tool::Upscale {
        upscale_dimensions(width, height)?;
    }

    let scratch = Scratch::new()?;
    let input = scratch.0.join("input.png");
    let result = scratch.0.join("result.png");
    let mask = scratch.0.join("mask.png");
    decoded
        .save_with_format(&input, image::ImageFormat::Png)
        .map_err(|error| format!("could not prepare the image: {error}"))?;
    check_cancelled(cancelled)?;

    let mut command = Command::new(&prepared.runtime);
    command
        .current_dir(prepared.runtime.parent().unwrap_or(Path::new(".")))
        .arg(prepared.tool.command())
        .arg("-m")
        .arg(&prepared.model)
        .arg("-i")
        .arg(&input);
    match prepared.tool {
        Tool::RemoveBackground => {
            command.arg("-o").arg(&mask).arg("--composite").arg(&result);
        }
        Tool::Upscale => {
            command
                .arg("-o")
                .arg(&result)
                .arg("--tile")
                .arg(ESRGAN_TILE);
        }
    }
    configure_library_path(&mut command, &prepared.runtime)?;
    let output = command
        .output()
        .map_err(|error| format!("could not start the local AI engine: {error}"))?;
    if !output.status.success() {
        return Err(command_error(&output));
    }
    check_cancelled(cancelled)?;

    validate_result(&result, prepared.tool, width, height)?;
    let encoded = std::fs::read(&result)
        .map_err(|error| format!("could not read the local AI result: {error}"))?;
    let written = output_path(root, out_dir, source, prepared.tool)?;
    check_cancelled(cancelled)?;
    convert::write_output(root, &written, &encoded)
        .map_err(|_| "could not safely write the local AI result".to_string())?;
    Ok(written)
}

pub fn upscale_dimensions(width: u32, height: u32) -> Result<(u32, u32), String> {
    let out_width = width
        .checked_mul(4)
        .ok_or_else(|| "this image is too large to upscale 4×".to_string())?;
    let out_height = height
        .checked_mul(4)
        .ok_or_else(|| "this image is too large to upscale 4×".to_string())?;
    if u64::from(out_width) * u64::from(out_height) > MAX_UPSCALED_PIXELS {
        return Err(format!(
            "4× would create a {out_width}×{out_height} image; use AI operations for this size"
        ));
    }
    Ok((out_width, out_height))
}

fn output_path(root: &Path, out_dir: &Path, source: &Path, tool: Tool) -> Result<PathBuf, String> {
    convert::ai_output_path(root, out_dir, source, tool.output_suffix(), "png")
}

fn validate_result(
    path: &Path,
    tool: Tool,
    source_width: u32,
    source_height: u32,
) -> Result<(), String> {
    let reader = image::ImageReader::open(path)
        .map_err(|error| format!("local AI produced no readable result: {error}"))?
        .with_guessed_format()
        .map_err(|error| format!("local AI produced an unknown image: {error}"))?;
    let decoder = reader
        .into_decoder()
        .map_err(|error| format!("local AI produced an invalid image: {error}"))?;
    let dimensions = decoder.dimensions();
    let expected = match tool {
        Tool::RemoveBackground => (source_width, source_height),
        Tool::Upscale => upscale_dimensions(source_width, source_height)?,
    };
    if dimensions != expected {
        return Err(format!(
            "local AI returned {}×{} instead of {}×{}",
            dimensions.0, dimensions.1, expected.0, expected.1
        ));
    }
    if tool == Tool::RemoveBackground && !decoder.color_type().has_alpha() {
        return Err("background removal returned an image without transparency".into());
    }
    Ok(())
}

fn data_dir() -> Result<PathBuf, String> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local/share"))
            })
    }
    .ok_or_else(|| "the platform did not provide a local data folder".to_string())?;
    Ok(base.join("imageguide").join("ai"))
}

fn vision_binary_name() -> &'static str {
    if cfg!(windows) {
        "vision-cli.exe"
    } else {
        "vision-cli"
    }
}

fn discover_runtime(base: &Path) -> Result<Option<PathBuf>, String> {
    if let Some(override_path) = std::env::var_os("PRESS_VISION_CLI") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return Ok(Some(path));
        }
        return Err(format!(
            "PRESS_VISION_CLI does not name a file: {}",
            path.display()
        ));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(bundled) = bundled_runtime(&executable)
    {
        return Ok(Some(bundled));
    }
    let cached = base
        .join(RUNTIME_DIR)
        .join("bin")
        .join(vision_binary_name());
    Ok(cached.is_file().then_some(cached))
}

fn bundled_runtime(executable: &Path) -> Option<PathBuf> {
    let parent = executable.parent()?;
    let adjacent = parent.join(vision_binary_name());
    if adjacent.is_file() {
        return Some(adjacent);
    }
    let resource = parent
        .parent()?
        .join("Resources/visioncpp/bin")
        .join(vision_binary_name());
    resource.is_file().then_some(resource)
}

fn ensure_runtime(base: &Path, cancelled: &AtomicBool) -> Result<PathBuf, String> {
    if let Some(runtime) = discover_runtime(base)? {
        return Ok(runtime);
    }
    provision_runtime(base, cancelled)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn provision_runtime(base: &Path, cancelled: &AtomicBool) -> Result<PathBuf, String> {
    let archive = ensure_asset(
        &base.join("downloads").join(RUNTIME.filename),
        RUNTIME,
        cancelled,
    )?;
    let destination = base.join(RUNTIME_DIR);
    let binary = destination.join("bin").join(vision_binary_name());
    if runtime_is_valid(&destination) {
        return Ok(binary);
    }

    let temporary = base.join(format!("{RUNTIME_DIR}.part-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temporary);
    std::fs::create_dir(&temporary).map_err(|error| format!("AI runtime setup: {error}"))?;
    let unpacked = (|| -> Result<(), String> {
        let file = File::open(&archive).map_err(|error| format!("AI runtime archive: {error}"))?;
        let mut tar = tar::Archive::new(GzDecoder::new(file));
        for entry in tar
            .entries()
            .map_err(|error| format!("AI runtime archive: {error}"))?
        {
            check_cancelled(cancelled)?;
            let mut entry = entry.map_err(|error| format!("AI runtime archive: {error}"))?;
            if !entry
                .unpack_in(&temporary)
                .map_err(|error| format!("AI runtime archive: {error}"))?
            {
                return Err("AI runtime archive contained an unsafe path".into());
            }
        }
        if !runtime_is_valid(&temporary) {
            return Err("AI runtime archive is incomplete".into());
        }
        if destination.exists() {
            std::fs::remove_dir_all(&destination)
                .map_err(|error| format!("AI runtime repair: {error}"))?;
        }
        std::fs::rename(&temporary, &destination)
            .map_err(|error| format!("AI runtime install: {error}"))?;
        Ok(())
    })();
    if unpacked.is_err() {
        let _ = std::fs::remove_dir_all(&temporary);
    }
    unpacked?;
    Ok(binary)
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn provision_runtime(_base: &Path, _cancelled: &AtomicBool) -> Result<PathBuf, String> {
    Err(
        "automatic vision.cpp 0.3.0 setup is not available on this platform; set PRESS_VISION_CLI to a local build"
            .to_string(),
    )
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn runtime_is_valid(root: &Path) -> bool {
    [
        root.join("bin").join(vision_binary_name()),
        root.join("bin/libggml-cpu.so"),
        root.join("lib/libvisioncpp.so"),
    ]
    .iter()
    .all(|path| path.is_file())
}

fn ensure_asset(path: &Path, asset: Asset, cancelled: &AtomicBool) -> Result<PathBuf, String> {
    if verify_asset(path, asset)? {
        return Ok(path.to_path_buf());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("AI download folder: {error}"))?;
    }
    if path.symlink_metadata().is_ok() {
        std::fs::remove_file(path).map_err(|error| format!("AI asset repair: {error}"))?;
    }

    let mut partial_name = path.as_os_str().to_os_string();
    partial_name.push(format!(".part-{}", std::process::id()));
    let partial = PathBuf::from(partial_name);
    let _ = std::fs::remove_file(&partial);
    let downloaded = (|| -> Result<(), String> {
        check_cancelled(cancelled)?;
        let response = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(120))
            .build()
            .get(asset.url)
            .call()
            .map_err(|error| format!("could not download {}: {error}", asset.filename))?;
        let mut reader = response.into_reader();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|error| format!("AI download: {error}"))?;
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            check_cancelled(cancelled)?;
            let read = reader
                .read(&mut chunk)
                .map_err(|error| format!("AI download: {error}"))?;
            if read == 0 {
                break;
            }
            bytes = bytes
                .checked_add(read as u64)
                .ok_or_else(|| "AI download is too large".to_string())?;
            if bytes > asset.bytes {
                return Err(format!("{} was larger than expected", asset.filename));
            }
            hasher.update(&chunk[..read]);
            file.write_all(&chunk[..read])
                .map_err(|error| format!("AI download: {error}"))?;
        }
        file.sync_all()
            .map_err(|error| format!("AI download: {error}"))?;
        let digest = format!("{:x}", hasher.finalize());
        if bytes != asset.bytes || digest != asset.sha256 {
            return Err(format!("{} failed its integrity check", asset.filename));
        }
        std::fs::rename(&partial, path).map_err(|error| format!("AI download: {error}"))?;
        Ok(())
    })();
    if downloaded.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    downloaded?;
    Ok(path.to_path_buf())
}

fn verify_asset(path: &Path, asset: Asset) -> Result<bool, String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() && metadata.len() == asset.bytes => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("AI asset: {error}")),
    }
    let mut file = File::open(path).map_err(|error| format!("AI asset: {error}"))?;
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut chunk)
            .map_err(|error| format!("AI asset: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()) == asset.sha256)
}

fn configure_library_path(command: &mut Command, runtime: &Path) -> Result<(), String> {
    let bin = runtime
        .parent()
        .ok_or_else(|| "the local AI runtime has no parent folder".to_string())?;
    let root = bin.parent().unwrap_or(bin);
    let variable = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else if cfg!(windows) {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut paths = vec![bin.to_path_buf(), root.join("lib")];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    let joined = std::env::join_paths(paths)
        .map_err(|error| format!("could not configure the local AI runtime: {error}"))?;
    command.env(variable, joined);
    Ok(())
}

fn command_error(output: &std::process::Output) -> String {
    let logs = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let detail = logs
        .split(['\n', '\r'])
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(300).collect::<String>())
        .unwrap_or_else(|| format!("engine exited with {}", output.status));
    format!("local AI failed: {detail}")
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("local AI stopped because the folder changed".into())
    } else {
        Ok(())
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        for _ in 0..100 {
            let id = SCRATCH_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("press-ai-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("could not create AI scratch space: {error}")),
            }
        }
        Err("could not reserve AI scratch space".into())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("press-local-ai-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn upscale_refuses_an_output_too_large_to_hold_safely() {
        assert_eq!(upscale_dimensions(640, 427).unwrap(), (2560, 1708));
        assert!(upscale_dimensions(5410, 3606).is_err());
    }

    #[test]
    fn ai_outputs_mirror_folders_and_never_claim_an_existing_name() {
        let root = temp_dir("paths");
        let source = root.join("products/chair.jpg");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"source").unwrap();
        let out_dir = root.join(scan::OUTPUT_DIR);
        let first = output_path(&root, &out_dir, &source, Tool::RemoveBackground).unwrap();
        assert_eq!(
            first,
            root.join("optimized/products/chair-background-removed.png")
        );
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"existing").unwrap();
        assert_eq!(
            output_path(&root, &out_dir, &source, Tool::RemoveBackground).unwrap(),
            root.join("optimized/products/chair-background-removed-2.png")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cached_assets_must_match_both_size_and_digest() {
        let root = temp_dir("digest");
        let path = root.join("asset");
        std::fs::write(&path, b"abc").unwrap();
        let asset = Asset {
            filename: "asset",
            url: "unused",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            bytes: 3,
        };
        assert!(verify_asset(&path, asset).unwrap());
        std::fs::write(&path, b"abd").unwrap();
        assert!(!verify_asset(&path, asset).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bundled_runtime_is_found_inside_a_macos_app() {
        let root = temp_dir("bundled-runtime");
        let executable = root.join("Press.app/Contents/MacOS/press");
        let runtime = root
            .join("Press.app/Contents/Resources/visioncpp/bin")
            .join(vision_binary_name());
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        std::fs::write(&runtime, b"runtime").unwrap();
        assert_eq!(bundled_runtime(&executable), Some(runtime));
        let _ = std::fs::remove_dir_all(root);
    }
}
