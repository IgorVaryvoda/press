//! Direct Sirv Studio image processing.
//!
//! Press uploads only the image a person explicitly runs, calls Studio's REST
//! API, then writes the returned image through the same safe output path as its
//! local models. Browser links are kept only for obtaining and managing a key.

use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::{convert, scan, settings, sirv};

const API: &str = "https://dev.sirv.studio";
pub const API_KEYS_URL: &str = "https://dev.sirv.studio/settings/api?utm_source=press&utm_medium=desktop&utm_campaign=studio-api-key";
const MAX_UPLOAD: u64 = 20 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROCESS_TIMEOUT: Duration = Duration::from_secs(360);
static BOUNDARY_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    ImageToImage,
    RemoveBackground,
    ReplaceBackground,
    Upscale,
    ProductLifestyle,
}

pub const TOOLS: &[Tool] = &[
    Tool::ImageToImage,
    Tool::RemoveBackground,
    Tool::ReplaceBackground,
    Tool::Upscale,
    Tool::ProductLifestyle,
];

impl Tool {
    pub fn slug(self) -> &'static str {
        match self {
            Self::ImageToImage => "image-to-image",
            Self::RemoveBackground => "background-removal",
            Self::ReplaceBackground => "background-replace",
            Self::Upscale => "upscale",
            Self::ProductLifestyle => "product-lifestyle",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ImageToImage => "Image to Image",
            Self::RemoveBackground => "Background Removal",
            Self::ReplaceBackground => "Background Replace",
            Self::Upscale => "Image Upscale 2×",
            Self::ProductLifestyle => "Product Lifestyle",
        }
    }

    pub fn needs_prompt(self) -> bool {
        matches!(
            self,
            Self::ImageToImage | Self::ReplaceBackground | Self::ProductLifestyle
        )
    }

    pub fn prompt_placeholder(self) -> &'static str {
        match self {
            Self::ImageToImage => "Describe the transformation",
            Self::ReplaceBackground => "Describe the new background",
            Self::ProductLifestyle => "Describe the product scene",
            Self::RemoveBackground | Self::Upscale => "",
        }
    }

    pub fn result_label(self) -> &'static str {
        match self {
            Self::ImageToImage => "AI transformation",
            Self::RemoveBackground => "AI background removal",
            Self::ReplaceBackground => "AI background replace",
            Self::Upscale => "AI upscale 2×",
            Self::ProductLifestyle => "AI lifestyle image",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::ImageToImage => "image-to-image",
            Self::RemoveBackground => "remove-bg",
            Self::ReplaceBackground => "replace-bg",
            Self::Upscale => "upscale",
            Self::ProductLifestyle => "product-lifestyle",
        }
    }

    fn output_suffix(self) -> &'static str {
        match self {
            Self::ImageToImage => "-studio-edit",
            Self::RemoveBackground => "-studio-background-removed",
            Self::ReplaceBackground => "-studio-background-replaced",
            Self::Upscale => "-studio-2x",
            Self::ProductLifestyle => "-studio-lifestyle",
        }
    }

    fn request(self, image_url: &str, prompt: &str) -> serde_json::Value {
        match self {
            Self::ImageToImage | Self::ReplaceBackground => {
                serde_json::json!({ "image_url": image_url, "prompt": prompt })
            }
            Self::ProductLifestyle => serde_json::json!({
                "image_url": image_url,
                "scene_description": prompt,
            }),
            Self::Upscale => serde_json::json!({ "image_url": image_url, "scale": 2 }),
            Self::RemoveBackground => serde_json::json!({ "image_url": image_url }),
        }
    }
}

#[derive(Deserialize)]
struct Uploaded {
    url: String,
}

#[derive(Deserialize)]
struct Processed {
    image_url: String,
}

/// The bytes Studio will receive. A prepared copy exists only in memory and is
/// never written beside either the source or the user's output.
pub struct PreparedUpload {
    bytes: Vec<u8>,
    mime: &'static str,
    extension: &'static str,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
impl PreparedUpload {
    pub(crate) fn for_test() -> Self {
        Self {
            bytes: Vec::new(),
            mime: "image/webp",
            extension: "webp",
            width: 1,
            height: 1,
        }
    }
}

pub enum Preflight {
    Ready(PreparedUpload),
    NeedsConfirmation(PreparedUpload),
}

struct Client {
    api: String,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(api: &str, key: &str) -> Result<Self, String> {
        validate_key(key)?;
        Ok(Self {
            api: api.trim_end_matches('/').to_string(),
            key: key.to_string(),
            agent: ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build(),
        })
    }

    fn authorization(&self) -> String {
        format!("Bearer {}", self.key)
    }

    fn verify(&self) -> Result<(), String> {
        self.agent
            .get(&format!("{}/api/zapier/me", self.api))
            .set("Authorization", &self.authorization())
            .call()
            .map(|_| ())
            .map_err(studio_error("verify key"))
    }

    fn upload(&self, upload: &PreparedUpload) -> Result<String, String> {
        let bytes = &upload.bytes;
        let mime = upload.mime;
        let extension = upload.extension;
        let boundary = format!(
            "press-{}-{}",
            std::process::id(),
            BOUNDARY_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut body = Vec::with_capacity(bytes.len() + 256);
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"upload.{extension}\"\r\nContent-Type: {mime}\r\n\r\n"
        )
        .map_err(|error| format!("could not prepare the Studio upload: {error}"))?;
        body.extend_from_slice(bytes);
        write!(body, "\r\n--{boundary}--\r\n")
            .map_err(|error| format!("could not prepare the Studio upload: {error}"))?;

        let response = self
            .agent
            .post(&format!("{}/api/zapier/upload", self.api))
            .set("Authorization", &self.authorization())
            .set(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .timeout(PROCESS_TIMEOUT)
            .send_bytes(&body)
            .map_err(studio_error("upload image"))?;
        response
            .into_json::<Uploaded>()
            .map(|uploaded| uploaded.url)
            .map_err(|error| format!("Studio returned an invalid upload response: {error}"))
    }

    fn process(&self, tool: Tool, image_url: &str, prompt: &str) -> Result<String, String> {
        let response = self
            .agent
            .post(&format!("{}/api/zapier/{}", self.api, tool.endpoint()))
            .set("Authorization", &self.authorization())
            .timeout(PROCESS_TIMEOUT)
            .send_json(tool.request(image_url, prompt))
            .map_err(studio_error(tool.label()))?;
        response
            .into_json::<Processed>()
            .map(|processed| processed.image_url)
            .map_err(|error| format!("Studio returned an invalid image response: {error}"))
    }

    fn download(&self, url: &str) -> Result<Vec<u8>, String> {
        if !result_url_allowed(url) {
            return Err("Studio returned an unsafe result URL".into());
        }
        let response = self
            .agent
            .get(url)
            .timeout(PROCESS_TIMEOUT)
            .call()
            .map_err(studio_error("download result"))?;
        if response
            .header("Content-Length")
            .and_then(|length| length.parse::<u64>().ok())
            .is_some_and(|length| length > sirv::MAX_TRANSFER)
        {
            return Err("Studio returned a result larger than Press's transfer cap".into());
        }
        sirv::read_capped(response.into_reader(), sirv::MAX_TRANSFER)
            .map_err(|error| format!("could not download the Studio result: {error}"))
    }
}

pub fn verify_key(key: &str) -> Result<(), String> {
    Client::new(API, key)?.verify()
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn process_with_api(
    api: &str,
    key: &str,
    root: &Path,
    out_dir: &Path,
    source: &Path,
    output_source: &Path,
    tool: Tool,
    prompt: &str,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    let upload = match prepare_upload(source, cancelled)? {
        Preflight::Ready(upload) => upload,
        Preflight::NeedsConfirmation(upload) => upload,
    };
    process_prepared_with_api(
        api,
        key,
        root,
        out_dir,
        &upload,
        output_source,
        tool,
        prompt,
        cancelled,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn process_prepared(
    key: &str,
    root: &Path,
    out_dir: &Path,
    upload: &PreparedUpload,
    output_source: &Path,
    tool: Tool,
    prompt: &str,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    process_prepared_with_api(
        API,
        key,
        root,
        out_dir,
        upload,
        output_source,
        tool,
        prompt,
        cancelled,
    )
}

#[allow(clippy::too_many_arguments)]
fn process_prepared_with_api(
    api: &str,
    key: &str,
    root: &Path,
    out_dir: &Path,
    upload: &PreparedUpload,
    output_source: &Path,
    tool: Tool,
    prompt: &str,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    check_cancelled(cancelled)?;
    if tool.needs_prompt() && prompt.trim().is_empty() {
        return Err(format!("{} needs a prompt", tool.label()));
    }
    let client = Client::new(api, key)?;
    let uploaded = client.upload(upload)?;
    check_cancelled(cancelled)?;
    let result_url = client.process(tool, &uploaded, prompt.trim())?;
    check_cancelled(cancelled)?;
    let bytes = client.download(&result_url)?;
    check_cancelled(cancelled)?;
    let extension = result_extension(&bytes)?;
    let written = convert::ai_output_path(
        root,
        out_dir,
        output_source,
        tool.output_suffix(),
        extension,
    )?;
    convert::write_output(root, &written, &bytes)
        .map_err(|_| "could not safely write the Studio result".to_string())?;
    Ok(written)
}

/// Build the smallest truthful upload path: accepted bytes pass through unchanged;
/// anything else first gets a pixel-preserving WebP copy. Only a lossy or resized
/// copy asks the user before it leaves this computer.
pub fn prepare_upload(source: &Path, cancelled: &AtomicBool) -> Result<Preflight, String> {
    prepare_upload_using(source, cancelled, MAX_UPLOAD)
}

fn prepare_upload_using(
    source: &Path,
    cancelled: &AtomicBool,
    max_upload: u64,
) -> Result<Preflight, String> {
    check_cancelled(cancelled)?;
    let source_bytes = std::fs::metadata(source)
        .map_err(|error| format!("could not read the source image: {error}"))?
        .len();
    if source_bytes <= max_upload {
        let bytes = std::fs::read(source)
            .map_err(|error| format!("could not read the source image: {error}"))?;
        if let Ok((mime, extension)) = upload_format(&bytes) {
            return Ok(Preflight::Ready(PreparedUpload {
                bytes,
                mime,
                extension,
                width: 0,
                height: 0,
            }));
        }
    }

    check_cancelled(cancelled)?;
    let (mut image, _profile) =
        scan::decode_for_conversion(source).map_err(|error| match error {
            scan::ConversionDecodeError::AnimatedGif => {
                "this animated GIF is too large for Studio without dropping its animation"
                    .to_string()
            }
            scan::ConversionDecodeError::AnimatedPng => {
                "this animated PNG is too large for Studio without dropping its animation"
                    .to_string()
            }
            scan::ConversionDecodeError::AnimatedWebP => {
                "this animated WebP is too large for Studio without dropping its animation"
                    .to_string()
            }
            scan::ConversionDecodeError::AnimatedJpegXl => {
                "this animated JPEG XL is too large for Studio without dropping its animation"
                    .to_string()
            }
            scan::ConversionDecodeError::Failed => {
                "Press could not decode this image to prepare it for Studio".to_string()
            }
        })?;
    check_cancelled(cancelled)?;

    let lossless = convert::encode(
        &image,
        convert::Format::WebP,
        convert::Quality::LOSSLESS,
        None,
    )
    .ok()
    .ok_or_else(|| "Press could not prepare a lossless Studio upload copy".to_string())?;
    if lossless.len() as u64 <= max_upload {
        return Ok(Preflight::Ready(PreparedUpload {
            bytes: lossless,
            mime: "image/webp",
            extension: "webp",
            width: image.width(),
            height: image.height(),
        }));
    }

    check_cancelled(cancelled)?;
    let mut encoded = convert::encode(
        &image,
        convert::Format::WebP,
        convert::Quality::lossy(90.),
        None,
    )
    .ok()
    .ok_or_else(|| "Press could not prepare a Studio upload copy".to_string())?;
    for _ in 0..8 {
        if encoded.len() as u64 <= max_upload {
            return Ok(Preflight::NeedsConfirmation(PreparedUpload {
                bytes: encoded,
                mime: "image/webp",
                extension: "webp",
                width: image.width(),
                height: image.height(),
            }));
        }
        check_cancelled(cancelled)?;
        let edge = image.width().max(image.height());
        if edge <= 1 {
            break;
        }
        let ratio = (max_upload as f64 / encoded.len() as f64).sqrt() * 0.95;
        let next = ((edge as f64 * ratio.min(0.9)).floor() as u32).max(1);
        image = convert::MaxEdge(Some(next)).apply(image);
        encoded = convert::encode(
            &image,
            convert::Format::WebP,
            convert::Quality::lossy(90.),
            None,
        )
        .ok()
        .ok_or_else(|| "Press could not prepare a Studio upload copy".to_string())?;
    }
    Err("Press could not prepare this image under Studio's 20 MB upload limit".into())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err("Studio run stopped".into())
    } else {
        Ok(())
    }
}

fn upload_format(bytes: &[u8]) -> Result<(&'static str, &'static str), String> {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => Ok(("image/jpeg", "jpg")),
        Ok(image::ImageFormat::Png) => Ok(("image/png", "png")),
        Ok(image::ImageFormat::Gif) => Ok(("image/gif", "gif")),
        Ok(image::ImageFormat::WebP) => Ok(("image/webp", "webp")),
        Ok(image::ImageFormat::Avif) => Ok(("image/avif", "avif")),
        Ok(image::ImageFormat::Bmp) => Ok(("image/bmp", "bmp")),
        Ok(image::ImageFormat::Tiff) => Ok(("image/tiff", "tiff")),
        Ok(image::ImageFormat::Ico) => Ok(("image/x-icon", "ico")),
        _ if bytes.starts_with(&[0xff, 0x0a])
            || bytes.starts_with(&[0, 0, 0, 12, b'J', b'X', b'L', b' ']) =>
        {
            Ok(("image/jxl", "jxl"))
        }
        _ => Err("Studio does not accept this image container".into()),
    }
}

fn result_extension(bytes: &[u8]) -> Result<&'static str, String> {
    if scan::decode_bytes(bytes).is_none() {
        return Err("Studio returned bytes that are not a readable image".into());
    }
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => Ok("jpg"),
        Ok(image::ImageFormat::Png) => Ok("png"),
        Ok(image::ImageFormat::Gif) => Ok("gif"),
        Ok(image::ImageFormat::WebP) => Ok("webp"),
        Ok(image::ImageFormat::Avif) => Ok("avif"),
        Ok(image::ImageFormat::Tiff) => Ok("tiff"),
        Ok(image::ImageFormat::Bmp) => Ok("bmp"),
        _ => Err("Studio returned an unsupported image format".into()),
    }
}

fn result_url_allowed(url: &str) -> bool {
    url.starts_with("https://") || cfg!(test) && url.starts_with("http://127.0.0.1:")
}

fn studio_error(stage: &'static str) -> impl Fn(ureq::Error) -> String {
    move |error| match error {
        ureq::Error::Status(status, response) => {
            let body = sirv::read_capped(response.into_reader(), 8 * 1024)
                .ok()
                .map(|body| String::from_utf8_lossy(&body).trim().to_string())
                .filter(|body| !body.is_empty())
                .unwrap_or_else(|| stage.to_string());
            studio_status_error(stage, status, &body)
        }
        ureq::Error::Transport(error) => format!("Studio {stage} failed: {error}"),
    }
}

fn studio_status_error(stage: &str, status: u16, body: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let detail = parsed
        .as_ref()
        .and_then(|body| {
            body.get("error")
                .or_else(|| body.get("message"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.to_string());
    let code = parsed
        .as_ref()
        .and_then(|body| body.get("code"))
        .and_then(|value| value.as_str());
    match status {
        401 => format!("Studio rejected the API key: {detail}"),
        402 if code == Some("INSUFFICIENT_CREDITS") => {
            format!("Studio has no credits available: {detail}")
        }
        402 => format!("Studio API access failed: {detail}"),
        403 => format!("Studio API access is not enabled for this workspace: {detail}"),
        413 => format!("Studio rejected the image as too large: {detail}"),
        429 => format!("Studio is rate limiting this account: {detail}"),
        _ => format!("Studio {stage} failed ({status}): {detail}"),
    }
}

fn validate_key(key: &str) -> Result<(), String> {
    if key.starts_with("sk_live_")
        && key.len() <= 512
        && !key.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        Ok(())
    } else {
        Err("Studio API keys start with sk_live_".into())
    }
}

fn store_path() -> Option<PathBuf> {
    settings::path().map(|path| path.with_file_name("studio"))
}

pub fn load_key() -> Option<String> {
    let text = std::fs::read_to_string(store_path()?).ok()?;
    let key = text.strip_prefix("api_key=")?.trim().to_string();
    validate_key(&key).ok()?;
    Some(key)
}

pub fn save_key(key: &str) -> Result<(), String> {
    validate_key(key)?;
    let path = store_path().ok_or_else(|| "no config directory on this system".to_string())?;
    save_key_to(&path, key)
}

fn save_key_to(path: &Path, key: &str) -> Result<(), String> {
    validate_key(key)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.part", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let _ = std::fs::remove_file(&temporary);
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        writeln!(file, "api_key={key}").map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| error.to_string())?;
        }
        replace_file(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub fn forget_key() -> Result<(), String> {
    let Some(path) = store_path() else {
        return Ok(());
    };
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(error) if to.exists() => {
            std::fs::remove_file(to)?;
            std::fs::rename(from, to).map_err(|_| error)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::{Cursor, Read};
    use std::net::{TcpListener, TcpStream};

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 2048];
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            request.extend_from_slice(&chunk[..read]);
            let Some(headers_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..headers_end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= headers_end + length {
                return request;
            }
        }
    }

    fn respond(mut stream: TcpStream, content_type: &str, body: &[u8]) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255])))
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "press-studio-{name}-{}-{}",
            std::process::id(),
            BOUNDARY_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn one_direct_run_uploads_processes_and_writes_a_real_result() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        let output = png();
        let server_api = api.clone();
        let server_output = output.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with("POST /api/zapier/upload "));
            assert!(text.contains("Authorization: Bearer sk_live_test"));
            assert!(text.contains("name=\"file\"; filename=\"upload.png\""));
            respond(
                stream,
                "application/json",
                br#"{"url":"https://upload.test/input.png"}"#,
            );

            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with("POST /api/zapier/replace-bg "));
            assert!(text.contains("Authorization: Bearer sk_live_test"));
            assert!(text.contains("\"prompt\":\"clean white studio\""));
            let body = format!("{{\"image_url\":\"{server_api}/result.png\"}}");
            respond(stream, "application/json", body.as_bytes());

            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with("GET /result.png "));
            assert!(!text.contains("Authorization:"));
            respond(stream, "image/png", &server_output);
        });

        let root = std::env::temp_dir().join(format!(
            "press-studio-test-{}-{}",
            std::process::id(),
            BOUNDARY_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("optimized")).unwrap();
        let output_source = root.join("photo.png");
        let source = root.join("optimized/photo.webp");
        std::fs::write(&source, png()).unwrap();
        let written = process_with_api(
            &api,
            "sk_live_test",
            &root,
            &root.join("optimized"),
            &source,
            &output_source,
            Tool::ReplaceBackground,
            "clean white studio",
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(
            written,
            root.join("optimized/photo-studio-background-replaced.png")
        );
        assert_eq!(std::fs::read(&written).unwrap(), output);
        server.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_saved_key_is_private_and_round_trips() {
        let root =
            std::env::temp_dir().join(format!("press-studio-key-test-{}", std::process::id()));
        let path = root.join("studio");
        save_key_to(&path, "sk_live_test").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "api_key=sk_live_test\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unsupported_container_is_prepared_losslessly_without_confirmation() {
        let path = scratch("unsupported.qoi");
        let image = RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 127]));
        DynamicImage::ImageRgba8(image.clone())
            .save_with_format(&path, ImageFormat::Qoi)
            .unwrap();

        let Preflight::Ready(upload) = prepare_upload(&path, &AtomicBool::new(false)).unwrap()
        else {
            panic!("a pixel-preserving copy must not ask for confirmation");
        };
        assert_eq!((upload.width, upload.height), (2, 2));
        assert_eq!(scan::decode_bytes(&upload.bytes).unwrap().to_rgba8(), image);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_lossy_or_resized_upload_copy_needs_confirmation() {
        let path = scratch("large.qoi");
        let image = RgbaImage::from_fn(256, 256, |x, y| {
            let seed = x.wrapping_mul(73) ^ y.wrapping_mul(151);
            Rgba([seed as u8, (seed >> 3) as u8, (x ^ y) as u8, 255])
        });
        DynamicImage::ImageRgba8(image)
            .save_with_format(&path, ImageFormat::Qoi)
            .unwrap();

        let Preflight::NeedsConfirmation(upload) =
            prepare_upload_using(&path, &AtomicBool::new(false), 8 * 1024).unwrap()
        else {
            panic!("a visibly changed upload copy must ask first");
        };
        assert!(upload.bytes.len() <= 8 * 1024);
        assert!(upload.width <= 256 && upload.height <= 256);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn upload_format_matches_the_current_studio_server_contract() {
        assert_eq!(
            upload_format(&[0xff, 0x0a, 0, 0]).unwrap(),
            ("image/jxl", "jxl")
        );
        let mut bmp = Vec::new();
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255])))
            .write_to(&mut Cursor::new(&mut bmp), ImageFormat::Bmp)
            .unwrap();
        assert_eq!(upload_format(&bmp).unwrap(), ("image/bmp", "bmp"));
    }

    #[test]
    fn only_an_insufficient_credit_code_says_no_credits() {
        let entitlement = studio_status_error(
            "upload image",
            402,
            r#"{"error":"Full API and MCP access is not enabled for this workspace.","code":"API_MCP_FULL_ENTITLEMENT_REQUIRED"}"#,
        );
        assert_eq!(
            entitlement,
            "Studio API access failed: Full API and MCP access is not enabled for this workspace."
        );

        let credits = studio_status_error(
            "Background Removal",
            402,
            r#"{"error":"Insufficient credits. Required: 10, Available: 0","code":"INSUFFICIENT_CREDITS"}"#,
        );
        assert!(credits.starts_with("Studio has no credits available:"));
    }
}
