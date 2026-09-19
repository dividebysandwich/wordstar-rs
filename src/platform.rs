//! Host-platform services that differ between the native terminal and the
//! browser: a monotonic clock, safe document writes and crash-recovery copies
//! (native), and file download + file-picker helpers (wasm).
//!
//! Native file *reading* stays inline in `app.rs`/`wordstar.rs` via `std::fs`;
//! the browser cannot touch the filesystem, so it downloads bytes and opens
//! files through an `<input type="file">` dialog instead.

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

/// Write `bytes` to `path` without ever leaving a truncated file behind: the data
/// goes to a temporary file in the same directory, is flushed to disk, and only
/// then renamed over the target. The previous version, if any, is kept as
/// `<name>.bak` (WordStar's backup file); failing to write the backup never
/// blocks the save itself. A symlinked document is written through to its target.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file_safely(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = fs::metadata(&path).ok();
    if let Some(meta) = &existing
        && meta.is_file()
    {
        let _ = fs::copy(&path, backup_path(&path));
    }
    write_atomically(&path, bytes, existing.map(|m| m.permissions()))
}

/// The WordStar-style backup file for `path`: `chapter.md` → `chapter.md.bak`.
#[cfg(not(target_arch = "wasm32"))]
pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Write `bytes` to a sibling temporary file, sync it, then rename it over `path`.
#[cfg(not(target_arch = "wasm32"))]
fn write_atomically(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.wsrs-tmp"));
    let result = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Some(perms) = permissions {
            let _ = fs::set_permissions(&tmp, perms);
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Where crash-recovery copies live (`~/.local/share/wordstar-rs/recovery` on
/// Linux). Disabled under `cargo test` so tests never touch the user's copies.
#[cfg(not(target_arch = "wasm32"))]
fn recovery_dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(dirs::data_local_dir()?.join("wordstar-rs").join("recovery"))
}

/// The recovery file for `doc` (or for an untitled document) inside `dir`: the
/// document's file name plus a hash of its absolute path, so two `chapter1.md`
/// files in different folders never share a recovery copy.
#[cfg(not(target_arch = "wasm32"))]
fn recovery_file_in(dir: &Path, doc: Option<&Path>) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let Some(doc) = doc else {
        return dir.join("untitled.md");
    };
    let abs = std::path::absolute(doc).unwrap_or_else(|_| doc.to_path_buf());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    abs.hash(&mut hasher);
    let stem: String = doc
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect();
    dir.join(format!("{stem}-{:016x}.md", hasher.finish()))
}

/// Store a crash-recovery copy of the working text for `doc`.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_recovery(doc: Option<&Path>, text: &str) -> io::Result<()> {
    let dir = recovery_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no recovery directory"))?;
    save_recovery_in(&dir, doc, text)
}

#[cfg(not(target_arch = "wasm32"))]
fn save_recovery_in(dir: &Path, doc: Option<&Path>, text: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    write_atomically(&recovery_file_in(dir, doc), text.as_bytes(), None)
}

/// The recovery copy for `doc`, if one exists: its text and when it was written.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_recovery(doc: Option<&Path>) -> Option<(String, SystemTime)> {
    load_recovery_in(&recovery_dir()?, doc)
}

#[cfg(not(target_arch = "wasm32"))]
fn load_recovery_in(dir: &Path, doc: Option<&Path>) -> Option<(String, SystemTime)> {
    let file = recovery_file_in(dir, doc);
    let written = fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    Some((fs::read_to_string(&file).ok()?, written))
}

/// Delete the recovery copy for `doc` (after a save, or when changes are
/// deliberately discarded).
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_recovery(doc: Option<&Path>) {
    if let Some(dir) = recovery_dir() {
        let _ = fs::remove_file(recovery_file_in(&dir, doc));
    }
}

/// The personal word list (`~/.config/wordstar-rs/words.txt` on Linux): words
/// added to the spelling dictionary, one per line. Not used under `cargo test`.
#[cfg(not(target_arch = "wasm32"))]
fn personal_words_file() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    Some(dirs::config_dir()?.join("wordstar-rs").join("words.txt"))
}

/// The words in the personal word list.
#[cfg(not(target_arch = "wasm32"))]
pub fn personal_words() -> Vec<String> {
    personal_words_file()
        .and_then(|f| fs::read_to_string(f).ok())
        .map(|text| text.lines().map(str::trim).filter(|w| !w.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Add `word` to the personal word list.
#[cfg(not(target_arch = "wasm32"))]
pub fn add_personal_word(word: &str) {
    let Some(file) = personal_words_file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{word}");
    }
}

/// localStorage key for the browser's personal word list.
#[cfg(target_arch = "wasm32")]
const PERSONAL_WORDS: &str = "wordstar-rs:words";

/// The words in the personal word list (browser: kept in localStorage).
#[cfg(target_arch = "wasm32")]
pub fn personal_words() -> Vec<String> {
    local_storage()
        .and_then(|s| s.get_item(PERSONAL_WORDS).ok().flatten())
        .map(|text| text.lines().filter(|w| !w.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Add `word` to the personal word list.
#[cfg(target_arch = "wasm32")]
pub fn add_personal_word(word: &str) {
    if let Some(storage) = local_storage() {
        let mut words = personal_words();
        words.push(word.to_string());
        let _ = storage.set_item(PERSONAL_WORDS, &words.join("\n"));
    }
}

/// Monotonic time in milliseconds. Used for the double-click window and the
/// incremental-preview time budget, replacing `std::time::Instant` (which is
/// unavailable on `wasm32-unknown-unknown`).
#[cfg(not(target_arch = "wasm32"))]
pub fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(target_arch = "wasm32")]
pub fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

// localStorage keys for the autosaved working document, so an accidental reload
// or navigation doesn't lose unsaved edits.
#[cfg(target_arch = "wasm32")]
const DRAFT_TEXT: &str = "wordstar-rs:draft:text";
#[cfg(target_arch = "wasm32")]
const DRAFT_PATH: &str = "wordstar-rs:draft:path";
#[cfg(target_arch = "wasm32")]
const DRAFT_MODIFIED: &str = "wordstar-rs:draft:modified";

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// Persist the working document (text, file name, modified flag) to the browser's
/// localStorage so it can be recovered on the next load.
#[cfg(target_arch = "wasm32")]
pub fn save_draft(text: &str, path: &str, modified: bool) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(DRAFT_TEXT, text);
        let _ = storage.set_item(DRAFT_PATH, path);
        let _ = storage.set_item(DRAFT_MODIFIED, if modified { "1" } else { "0" });
    }
}

/// Load a previously autosaved draft, if any: `(text, file name, modified)`.
#[cfg(target_arch = "wasm32")]
pub fn load_draft() -> Option<(String, String, bool)> {
    let storage = local_storage()?;
    let text = storage.get_item(DRAFT_TEXT).ok().flatten()?;
    let path = storage.get_item(DRAFT_PATH).ok().flatten().unwrap_or_default();
    let modified = storage.get_item(DRAFT_MODIFIED).ok().flatten().as_deref() == Some("1");
    Some((text, path, modified))
}

/// Trigger a browser download of `bytes` as `filename` with the given MIME type.
#[cfg(target_arch = "wasm32")]
pub fn download(filename: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{Blob, BlobPropertyBag, HtmlAnchorElement, Url};

    let err = |m: &str| -> String { format!("download failed: {m}") };

    // Copy the bytes into a JS Uint8Array wrapped in an array for the Blob ctor.
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let opts = BlobPropertyBag::new();
    opts.set_type(mime);
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|_| err("Blob::new"))?;
    let url = Url::create_object_url_with_blob(&blob).map_err(|_| err("create_object_url"))?;

    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| err("no document"))?;
    let anchor: HtmlAnchorElement = document
        .create_element("a")
        .map_err(|_| err("create <a>"))?
        .dyn_into()
        .map_err(|_| err("cast <a>"))?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    // Keep it out of layout; click programmatically to start the download.
    anchor
        .style()
        .set_property("display", "none")
        .map_err(|_| err("style"))?;
    let body = document.body().ok_or_else(|| err("no body"))?;
    body.append_child(&anchor).map_err(|_| err("append"))?;
    anchor.click();
    let _ = body.remove_child(&anchor);
    let _ = Url::revoke_object_url(&url);
    let _: JsValue = JsValue::NULL;
    Ok(())
}

// A single-slot mailbox bridging the asynchronous file picker to the
// synchronous render loop: `stash_open` is called from the FileReader callback,
// and the loop drains it with `take_open` once per frame.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static PENDING_OPEN: std::cell::RefCell<Option<(String, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
}

/// Store a picked file for the render loop to consume. Latest pick wins.
#[cfg(target_arch = "wasm32")]
pub fn stash_open(name: String, bytes: Vec<u8>) {
    PENDING_OPEN.with(|p| *p.borrow_mut() = Some((name, bytes)));
}

/// Take the most recently picked file, if any.
#[cfg(target_arch = "wasm32")]
pub fn take_open() -> Option<(String, Vec<u8>)> {
    PENDING_OPEN.with(|p| p.borrow_mut().take())
}

/// Open the host's file picker (filtered to `.ws`/`.md`) and, once the user
/// chooses a file, invoke `on_loaded(name, bytes)`. Reading is asynchronous; the
/// callback runs later on the browser's event loop.
#[cfg(target_arch = "wasm32")]
pub fn pick_file<F>(on_loaded: F)
where
    F: Fn(String, Vec<u8>) + Clone + 'static,
{
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{FileReader, HtmlInputElement};

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(input) = document.create_element("input") else {
        return;
    };
    let input: HtmlInputElement = match input.dyn_into() {
        Ok(i) => i,
        Err(_) => return,
    };
    input.set_type("file");
    input.set_accept(".ws,.md,text/markdown,text/plain");

    // On `change`, read the first selected file as an ArrayBuffer.
    let input_for_change = input.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        let Some(files) = input_for_change.files() else {
            return;
        };
        let Some(file) = files.get(0) else {
            return;
        };
        let name = file.name();
        let reader = match FileReader::new() {
            Ok(r) => r,
            Err(_) => return,
        };
        let reader_for_load = reader.clone();
        let on_loaded = on_loaded.clone();
        let on_load = Closure::<dyn FnMut()>::new(move || {
            if let Ok(buf) = reader_for_load.result() {
                let array = js_sys::Uint8Array::new(&buf);
                on_loaded(name.clone(), array.to_vec());
            }
        });
        reader.set_onload(Some(on_load.as_ref().unchecked_ref()));
        let _ = reader.read_as_array_buffer(&file);
        on_load.forget();
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();

    input.click();
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wsrs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn safe_write_keeps_backup_and_leaves_no_temp_file() {
        let dir = scratch("safe-write");
        let doc = dir.join("story.md");
        write_file_safely(&doc, b"first draft\n").unwrap();
        assert!(!backup_path(&doc).exists(), "no backup for a brand-new file");
        write_file_safely(&doc, b"second draft\n").unwrap();
        assert_eq!(fs::read_to_string(&doc).unwrap(), "second draft\n");
        assert_eq!(fs::read_to_string(backup_path(&doc)).unwrap(), "first draft\n");
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|n| !n.ends_with(".wsrs-tmp")), "temp left: {names:?}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recovery_round_trip_is_keyed_by_document() {
        let dir = scratch("recovery");
        let a = Path::new("/novel/one/chapter1.md");
        let b = Path::new("/novel/two/chapter1.md");
        save_recovery_in(&dir, Some(a), "draft A").unwrap();
        save_recovery_in(&dir, None, "untitled draft").unwrap();
        assert_eq!(load_recovery_in(&dir, Some(a)).unwrap().0, "draft A");
        assert!(load_recovery_in(&dir, Some(b)).is_none(), "same name, other folder");
        assert_eq!(load_recovery_in(&dir, None).unwrap().0, "untitled draft");
        fs::remove_dir_all(&dir).ok();
    }
}
