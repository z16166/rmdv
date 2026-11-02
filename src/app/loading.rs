//! Async document, workspace, and image loading with size limits.

use super::*;

pub(super) async fn load_file(p: PathBuf) -> Result<(PathBuf, String), String> {
    let p = canonicalize_existing_path(p);
    #[cfg(feature = "pdf")]
    if p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        let path = p.clone();
        // PDFium is sync + holds a process-global lock; run off the async runtime.
        let md = tokio::task::spawn_blocking(move || crate::pdf::pdf_to_markdown(&path))
            .await
            .map_err(|e| e.to_string())??;
        return Ok((p, md));
    }
    let bytes = read_document_bytes(&p).await?;
    Ok((p, document_text(bytes)))
}

/// Synchronously read a non-PDF document for the initial launch frame, so
/// the first `view()` already renders the file instead of flashing the
/// welcome card. PDF files still use the async path (pdfium is heavy and
/// process-global). A single markdown read is fast and runs on the app's
/// init thread before the window paints.
pub(super) fn load_file_sync(p: &Path) -> Result<(PathBuf, String), String> {
    let p = canonicalize_existing_path(p.to_path_buf());
    let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(too_large_message(bytes.len() as u64));
    }
    Ok((p, document_text(bytes)))
}

/// Largest document the main viewer will read. Files past this are refused
/// before reading instead of being pulled whole into memory.
pub(super) const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

pub(super) async fn read_document_bytes(p: &Path) -> Result<Vec<u8>, String> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(p).await.map_err(|e| e.to_string())?;
    let len = file.metadata().await.map_err(|e| e.to_string())?.len();
    if len > MAX_DOCUMENT_BYTES {
        return Err(too_large_message(len));
    }
    // The size can change between the check and the read; never read past
    // the cap either way.
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(too_large_message(bytes.len() as u64));
    }
    Ok(bytes)
}

pub(super) fn too_large_message(len: u64) -> String {
    format!(
        "File is too large to open ({} MB; the limit is {} MB)",
        len / (1024 * 1024),
        MAX_DOCUMENT_BYTES / (1024 * 1024)
    )
}

/// Decode without copying valid UTF-8; only invalid input pays for a lossy
/// conversion.
pub(super) fn document_text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// Build the tree and file-finder index together on a blocking worker. The
/// tree-side entry/file budgets guarantee a very large project cannot grow
/// this task without bound.
pub(super) async fn load_workspace_snapshot(
    path: PathBuf,
    show_hidden: bool,
) -> Result<(PathBuf, tree::WorkspaceSnapshot), String> {
    let scan_path = path.clone();
    let snapshot =
        tokio::task::spawn_blocking(move || tree::build_workspace(&scan_path, show_hidden))
            .await
            .map_err(|error| error.to_string())??;
    Ok((path, snapshot))
}

pub(super) async fn load_full_mindmap_folder(
    path: PathBuf,
    show_hidden: bool,
) -> Result<(PathBuf, tree::ExpandedFolderSnapshot), String> {
    let scan_path = path.clone();
    let snapshot =
        tokio::task::spawn_blocking(move || tree::load_expanded_folder(&scan_path, show_hidden))
            .await
            .map_err(|error| error.to_string())??;
    Ok((path, snapshot))
}

/// Remote images larger than this are refused instead of buffered whole.
pub(super) const MAX_REMOTE_IMAGE_BYTES: usize = 25 * 1024 * 1024;

/// Concurrent remote image downloads; a document with many images queues the
/// rest instead of opening one connection per image at once.
pub(super) const REMOTE_IMAGE_CONCURRENCY: usize = 4;

/// One HTTP client for every image fetch, so TLS setup and pooled connections
/// are reused. Iced drives all tasks on a single runtime, which the pool needs.
///
/// A connect that hangs is abandoned after `REMOTE_IMAGE_CONNECT_TIMEOUT`
/// and retried: from some networks the first connection to a host stalls
/// until the whole request times out, while the next one succeeds quickly.
pub(super) static IMAGE_CLIENT: std::sync::LazyLock<Result<reqwest::Client, String>> =
    std::sync::LazyLock::new(|| {
        reqwest::Client::builder()
            .connect_timeout(REMOTE_IMAGE_CONNECT_TIMEOUT)
            .timeout(REMOTE_IMAGE_TIMEOUT)
            .user_agent(concat!("rmdv/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())
    });

const REMOTE_IMAGE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const REMOTE_IMAGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
const REMOTE_IMAGE_ATTEMPTS: u32 = 3;
/// A cached remote image is used as-is for this long; after that a fresh
/// download is tried, and the cached bytes are kept if the download fails.
const REMOTE_IMAGE_CACHE_FRESH: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

pub(super) static IMAGE_FETCH_PERMITS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(REMOTE_IMAGE_CONCURRENCY));

pub(super) async fn fetch_image(url: String) -> (String, Result<Vec<u8>, String>) {
    if let Some(bytes) = cached_remote(&url, REMOTE_IMAGE_CACHE_FRESH).await {
        return (url, Ok(bytes));
    }
    let mut last = String::from("image request failed");
    for attempt in 0..REMOTE_IMAGE_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(300 * u64::from(attempt))).await;
        }
        match fetch_image_once(&url).await {
            Ok(bytes) => {
                store_cached_remote(&url, &bytes).await;
                return (url, Ok(bytes));
            }
            Err(err) if err.retry && attempt + 1 < REMOTE_IMAGE_ATTEMPTS => last = err.message,
            Err(err) => {
                last = err.message;
                break;
            }
        }
    }
    // A stale copy beats a blank failure when the host is unreachable.
    if let Some(bytes) = cached_remote(&url, std::time::Duration::MAX).await {
        return (url, Ok(bytes));
    }
    (url, Err(last))
}

struct FetchError {
    message: String,
    retry: bool,
}

async fn fetch_image_once(url: &str) -> Result<Vec<u8>, FetchError> {
    let client = IMAGE_CLIENT.as_ref().map_err(|e| FetchError {
        message: e.clone(),
        retry: false,
    })?;
    let _permit = IMAGE_FETCH_PERMITS
        .acquire()
        .await
        .map_err(|e| FetchError {
            message: e.to_string(),
            retry: false,
        })?;
    let mut resp = client.get(url).send().await.map_err(fetch_error)?;
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        return Err(FetchError {
            message: format!("http {code}"),
            retry: matches!(code, 408 | 429 | 500 | 502 | 503 | 504),
        });
    }
    let too_large = || FetchError {
        message: format!("image larger than {} MB", MAX_REMOTE_IMAGE_BYTES >> 20),
        retry: false,
    };
    let declared = resp.content_length().unwrap_or(0) as usize;
    if declared > MAX_REMOTE_IMAGE_BYTES {
        return Err(too_large());
    }
    // Stream so an undeclared or lying length still stops at the cap.
    let mut bytes = Vec::with_capacity(declared);
    while let Some(chunk) = resp.chunk().await.map_err(fetch_error)? {
        if bytes.len() + chunk.len() > MAX_REMOTE_IMAGE_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Timeouts, dropped connections and transient HTTP statuses are worth
/// another attempt. A 404 is not.
fn fetch_error(err: reqwest::Error) -> FetchError {
    let retry = err.is_timeout()
        || err.is_connect()
        || err.is_decode()
        || err.is_request()
        || matches!(
            err.status().map(|status| status.as_u16()),
            Some(408 | 429 | 500 | 502 | 503 | 504)
        );
    let mut message = err.to_string();
    let mut source = std::error::Error::source(&err);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    FetchError { message, retry }
}

fn remote_cache_path(url: &str) -> Option<std::path::PathBuf> {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Some(
        dirs::cache_dir()?
            .join("rmdv")
            .join("images")
            .join(format!("{hash:016x}-{}", url.len())),
    )
}

/// `max_age` bounds how old a cached download may be. `Duration::MAX` keeps
/// any age, which is the fallback when a fresh download fails.
async fn cached_remote(url: &str, max_age: std::time::Duration) -> Option<Vec<u8>> {
    let path = remote_cache_path(url)?;
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if meta
        .modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age > max_age)
    {
        return None;
    }
    decode_cached_remote(&tokio::fs::read(&path).await.ok()?, url)
}

fn decode_cached_remote(data: &[u8], url: &str) -> Option<Vec<u8>> {
    let len = u32::from_le_bytes(data.get(..4)?.try_into().ok()?) as usize;
    let rest = data.get(4..)?;
    let (stored, bytes) = rest.split_at_checked(len)?;
    if stored != url.as_bytes() || bytes.is_empty() {
        return None;
    }
    Some(bytes.to_vec())
}

async fn store_cached_remote(url: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let Some(path) = remote_cache_path(url) else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if tokio::fs::create_dir_all(dir).await.is_err() {
        return;
    }
    let mut payload = Vec::with_capacity(4 + url.len() + bytes.len());
    payload.extend_from_slice(&(url.len() as u32).to_le_bytes());
    payload.extend_from_slice(url.as_bytes());
    payload.extend_from_slice(bytes);
    let tmp = path.with_extension("tmp");
    if tokio::fs::write(&tmp, &payload).await.is_err() {
        return;
    }
    // Windows refuses to rename over a file that already exists.
    let _ = tokio::fs::remove_file(&path).await;
    if tokio::fs::rename(&tmp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}

/// Local images larger than this are not loaded.
pub(super) const MAX_LOCAL_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Cache key for a document's local image: its resolved path. The zoom modal
/// opens the same key.
pub(super) fn local_image_key(url: &str, current_file: Option<&std::path::Path>) -> Option<String> {
    resolve_image_path(url, current_file).map(|path| path.to_string_lossy().into_owned())
}

/// Read a local image off the UI thread, keyed like `fetch_image` results.
pub(super) async fn read_local_image(key: String) -> (String, Result<Vec<u8>, String>) {
    let res = async {
        let path = std::path::Path::new(&key);
        let len = tokio::fs::metadata(path)
            .await
            .map_err(|e| e.to_string())?
            .len();
        if len > MAX_LOCAL_IMAGE_BYTES {
            return Err(format!(
                "image larger than {} MB",
                MAX_LOCAL_IMAGE_BYTES >> 20
            ));
        }
        tokio::fs::read(path).await.map_err(|e| e.to_string())
    }
    .await;
    (key, res)
}

/// Rasterize SVG bytes to RGBA. Target ~2048px on the longer side.
pub fn rasterize_svg(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    use resvg::tiny_skia;
    use resvg::usvg;
    const TARGET: f32 = 2048.0;
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| e.to_string())?;
    let sz = tree.size();
    let (w, h) = (sz.width(), sz.height());
    if w <= 0.0 || h <= 0.0 {
        return Err("svg has zero size".into());
    }
    let scale = (TARGET / w.max(h)).max(1.0);
    let pw = (w * scale).round() as u32;
    let ph = (h * scale).round() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph).ok_or("pixmap alloc failed")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok((pixmap.take(), pw, ph))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_test_url() -> String {
        format!(
            "http://127.0.0.1:1/rmdv-image-cache-{}-{}",
            std::process::id(),
            REMOTE_IMAGE_CACHE_FRESH.as_secs()
        )
    }

    #[tokio::test]
    async fn a_fresh_cached_image_is_reused_without_connecting() {
        let url = cache_test_url();
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".to_vec();
        store_cached_remote(&url, &bytes).await;
        let (_, got) = fetch_image(url.clone()).await;
        let _ = tokio::fs::remove_file(remote_cache_path(&url).unwrap()).await;
        assert_eq!(got.as_deref(), Ok(bytes.as_slice()));
    }

    #[tokio::test]
    async fn a_stale_cached_image_is_kept_when_the_download_fails() {
        let url = format!("{}-stale", cache_test_url());
        let bytes = b"png".to_vec();
        store_cached_remote(&url, &bytes).await;
        let path = remote_cache_path(&url).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(
            std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 60 * 60),
        )
        .unwrap();
        drop(file);
        let started = std::time::Instant::now();
        let (_, got) = fetch_image(url).await;
        let _ = tokio::fs::remove_file(&path).await;
        assert_eq!(got.as_deref(), Ok(bytes.as_slice()));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "a refused connection is not retried until the request timeout"
        );
    }

    #[test]
    fn a_cache_entry_for_another_url_is_ignored() {
        let data = {
            let url = "https://example.test/a.png";
            let mut data = (url.len() as u32).to_le_bytes().to_vec();
            data.extend_from_slice(url.as_bytes());
            data.extend_from_slice(b"image");
            data
        };
        assert!(decode_cached_remote(&data, "https://example.test/b.png").is_none());
        assert_eq!(
            decode_cached_remote(&data, "https://example.test/a.png").as_deref(),
            Some(&b"image"[..])
        );
    }
}

pub fn is_svg_bytes(b: &[u8]) -> bool {
    let head = &b[..b.len().min(512)];
    let s = std::str::from_utf8(head).unwrap_or("");
    let s = s.trim_start();
    s.starts_with("<svg") || s.starts_with("<?xml")
}
