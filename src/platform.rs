//! Host-platform services that differ between the native terminal and the
//! browser: a monotonic clock, and (on wasm) file download + file-picker helpers.
//!
//! Native file *reading*/*writing* stays inline in `app.rs`/`wordstar.rs` via
//! `std::fs`; the browser cannot touch the filesystem, so it downloads bytes and
//! opens files through an `<input type="file">` dialog instead.

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
