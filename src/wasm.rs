//! Browser entry point: boots the editor on a Ratzilla DOM backend.
//!
//! The native binary owns its own terminal loop (`main.rs`); here Ratzilla
//! drives rendering through `requestAnimationFrame` and delivers DOM keyboard /
//! mouse events. The editor state lives in an `Rc<RefCell<App>>` shared between
//! the render callback and the event handlers. The graphical document preview
//! is painted to an HTML `<canvas>` overlay (see [`canvas`]) since terminal
//! image protocols do not exist in a browser.

use std::cell::RefCell;
use std::rc::Rc;

use ratzilla::{DomBackend, WebRenderer};
use ratzilla::ratatui::Terminal;

use crate::app::App;
use crate::ui;

/// wasm entry point, invoked automatically when the module loads.
///
/// Ratzilla measures the terminal cell size once, when the `DomBackend` is
/// constructed, from the currently-active font — sized as `width: 1ch`, i.e. the
/// font's "0"-glyph advance. Web fonts load asynchronously, so if we initialized
/// immediately the grid would be sized for the fallback font and then end up with
/// too many columns once BigBlue Terminal (wider cell) swapped in, overflowing
/// the right edge. We therefore (1) wait for the screen font to load, then (2)
/// yield one animation frame so the browser actually applies it in a layout pass,
/// and only then build the backend so the single measurement uses the real font.
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        wait_for_screen_font().await;
        next_animation_frame().await;
        if let Err(err) = run() {
            web_sys::console::error_1(&err);
        }
    });
}

/// Resolve once the BigBlue Terminal screen font is loaded (or immediately if it
/// cannot be loaded), so the subsequent cell-size measurement is accurate.
async fn wait_for_screen_font() {
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        let promise = document.fonts().load("16px \"BigBlue Terminal\"");
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }
}

/// Await a single `requestAnimationFrame` tick, letting the browser run a layout
/// pass (so a just-loaded font is applied before anything measures the DOM).
async fn next_animation_frame() {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    let Some(window) = web_sys::window() else {
        return;
    };
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let cb = Closure::once_into_js(move || {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        });
        let _ = window.request_animation_frame(cb.unchecked_ref());
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Terminal cell size in CSS pixels. Must match the metrics pinned in
/// `index.html` (font-size 15px → 10px wide; line-height 20px tall). Ratzilla
/// also assumes this 10x20 cell internally, so keeping it in sync here lets us
/// map mouse pixel coordinates to grid cells directly.
const CELL_W_PX: f64 = 10.0;
const CELL_H_PX: f64 = 20.0;

/// Build the backend, wire up events and start the render loop.
fn run() -> Result<(), wasm_bindgen::JsValue> {
    let app = Rc::new(RefCell::new(
        App::new(None).map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?,
    ));

    // Recover the document autosaved before the last reload / navigation.
    if let Some((text, path, modified)) = crate::platform::load_draft()
        && !text.is_empty()
    {
        app.borrow_mut().restore_draft(&text, &path, modified);
    }

    let window = web_sys::window().ok_or_else(|| wasm_bindgen::JsValue::from_str("no window"))?;

    let backend = DomBackend::new().map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;
    let terminal =
        Terminal::new(backend).map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;

    // Ratzilla attaches its key/mouse listeners to the grid element, which it
    // destroys and rebuilds on every window resize — leaving input dead. So we
    // bind our own listeners to `window` instead; those survive grid rebuilds.
    install_input_handlers(&window, &app)?;

    // Final autosave when the page is being closed/reloaded, so even edits made
    // since the last periodic save are recovered.
    install_unload_autosave(&window, &app)?;

    // Render loop: advance async work, draw the TUI, periodically autosave, then
    // paint the preview canvas on top (or hide it).
    terminal.draw_web(move |frame| {
        let mut app = app.borrow_mut();
        if app.preview_loading() {
            app.step_preview_job();
        }
        app.poll_pending_open();
        ui::draw(frame, &app);
        canvas::render(&app);
        maybe_autosave(&app);
    });

    Ok(())
}

/// Persist the document to localStorage at most every `AUTOSAVE_INTERVAL_MS`, and
/// only when it has actually changed since the last save.
fn maybe_autosave(app: &App) {
    use std::cell::Cell;
    use std::hash::{Hash, Hasher};

    const AUTOSAVE_INTERVAL_MS: f64 = 1500.0;
    thread_local! {
        static LAST_MS: Cell<f64> = const { Cell::new(f64::NEG_INFINITY) };
        static LAST_HASH: Cell<u64> = const { Cell::new(0) };
    }

    let now = crate::platform::now_ms();
    if now - LAST_MS.with(Cell::get) < AUTOSAVE_INTERVAL_MS {
        return;
    }
    LAST_MS.with(|c| c.set(now));

    let text = app.document_text();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    app.modified.hash(&mut hasher);
    let hash = hasher.finish();
    if LAST_HASH.with(Cell::get) == hash {
        return;
    }
    LAST_HASH.with(|c| c.set(hash));
    crate::platform::save_draft(&text, &app.draft_path(), app.modified);
}

/// Register a `beforeunload` listener that saves the current document, catching
/// edits made since the last periodic autosave.
fn install_unload_autosave(
    window: &web_sys::Window,
    app: &Rc<RefCell<App>>,
) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let app = app.clone();
    let cb = Closure::<dyn FnMut()>::new(move || {
        if let Ok(app) = app.try_borrow() {
            crate::platform::save_draft(&app.document_text(), &app.draft_path(), app.modified);
        }
    });
    window.add_event_listener_with_callback("beforeunload", cb.as_ref().unchecked_ref())?;
    cb.forget();
    Ok(())
}

/// Bind keyboard and mouse listeners to `window` (not the grid), translating web
/// events into the editor's input vocabulary.
fn install_input_handlers(
    window: &web_sys::Window,
    app: &Rc<RefCell<App>>,
) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    // Keyboard. Reuse Ratzilla's `web_sys::KeyboardEvent` → key conversion. The
    // editor is a full-screen app that drives everything from the keyboard
    // (WordStar chords, F-keys), so suppress the browser's own handling — e.g.
    // F5 (reload), Ctrl-O/P/S (open/print/save dialogs) — which would otherwise
    // hijack keys the editor needs.
    let key_app = app.clone();
    let key_cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |ev: web_sys::KeyboardEvent| {
        let event = ratzilla::event::KeyEvent::from(ev.clone());
        // Bare modifier presses (Shift/Ctrl/Alt) and other keys Ratzilla can't
        // classify arrive as `Unidentified`; ignore them so they neither inject a
        // character nor swallow the browser's own handling of that key.
        if event.code == ratzilla::event::KeyCode::Unidentified {
            return;
        }
        ev.prevent_default();
        key_app.borrow_mut().handle_key(event.into());
    });
    window.add_event_listener_with_callback("keydown", key_cb.as_ref().unchecked_ref())?;
    key_cb.forget();

    // Mouse. The pinned 10x20 cell makes pixel → grid mapping a plain division.
    use crate::input::{MouseButton, MouseEventKind};
    install_mouse(window, app, "mousedown", |ev| {
        (ev.button() == 0).then_some(MouseEventKind::Down(MouseButton::Left))
    })?;
    install_mouse(window, app, "mouseup", |ev| {
        (ev.button() == 0).then_some(MouseEventKind::Up(MouseButton::Left))
    })?;
    install_mouse(window, app, "mousemove", |ev| {
        Some(if ev.buttons() & 1 != 0 {
            MouseEventKind::Drag(MouseButton::Left)
        } else {
            MouseEventKind::Moved
        })
    })?;
    Ok(())
}

/// Register one `window` mouse listener for `event_type`, turning the web event
/// into a [`crate::input::MouseEvent`] via `make_kind` (which decides the kind or
/// returns `None` to ignore the event).
fn install_mouse(
    window: &web_sys::Window,
    app: &Rc<RefCell<App>>,
    event_type: &str,
    make_kind: fn(&web_sys::MouseEvent) -> Option<crate::input::MouseEventKind>,
) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use crate::input::{KeyModifiers, MouseEvent};

    let app = app.clone();
    let cb = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |ev: web_sys::MouseEvent| {
        let Some(kind) = make_kind(&ev) else {
            return;
        };
        let column = (ev.client_x().max(0) as f64 / CELL_W_PX) as u16;
        let row = (ev.client_y().max(0) as f64 / CELL_H_PX) as u16;
        let mut modifiers = KeyModifiers::NONE;
        if ev.ctrl_key() {
            modifiers |= KeyModifiers::CONTROL;
        }
        if ev.alt_key() {
            modifiers |= KeyModifiers::ALT;
        }
        if ev.shift_key() {
            modifiers |= KeyModifiers::SHIFT;
        }
        app.borrow_mut().handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers,
        });
    });
    window.add_event_listener_with_callback(event_type, cb.as_ref().unchecked_ref())?;
    cb.forget();
    Ok(())
}

/// The full-window `<canvas>` that shows the rasterized document preview.
mod canvas {
    use std::cell::RefCell;

    use wasm_bindgen::{Clamped, JsCast};
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

    use crate::app::{App, Mode};

    const CANVAS_ID: &str = "ws-preview-canvas";
    // Identifies a rendered view so we only repaint when something changes,
    // rather than every animation frame.
    type ViewKey = (usize, usize, i32, i32, i32, u32, u32);

    thread_local! {
        static LAST: RefCell<Option<ViewKey>> = const { RefCell::new(None) };
    }

    /// Show the current preview page on the overlay canvas, or hide it when the
    /// preview is not active.
    pub fn render(app: &App) {
        let active = app.mode == Mode::Preview && !app.preview_pages.is_empty();
        if !active {
            hide();
            return;
        }
        let Some(page) = app.preview_pages.get(app.preview_page) else {
            hide();
            return;
        };

        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let vw = window.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        let vh = window.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(0.0);
        if vw < 1.0 || vh < 1.0 {
            return;
        }
        let (cw, ch) = (vw as u32, vh as u32);

        // Repaint only when the page / zoom / pan / viewport actually changes.
        let key: ViewKey = (
            app.preview_pages.as_ptr() as usize,
            app.preview_page,
            (app.preview_zoom * 1000.0) as i32,
            (app.preview_off.0 * 1000.0) as i32,
            (app.preview_off.1 * 1000.0) as i32,
            cw,
            ch,
        );
        if LAST.with(|l| *l.borrow() == Some(key)) {
            return;
        }

        let canvas = match ensure_canvas(&document) {
            Some(c) => c,
            None => return,
        };
        canvas.set_width(cw);
        canvas.set_height(ch);
        let _ = canvas.style().set_property("display", "block");
        let _ = canvas.style().set_property("width", &format!("{vw}px"));
        let _ = canvas.style().set_property("height", &format!("{vh}px"));

        let Ok(Some(obj)) = canvas.get_context("2d") else {
            return;
        };
        let Ok(ctx) = obj.dyn_into::<CanvasRenderingContext2d>() else {
            return;
        };

        // Dark backdrop behind the (letterboxed) page.
        ctx.set_fill_style_str("#1a1a1a");
        ctx.fill_rect(0.0, 0.0, cw as f64, ch as f64);

        let pw = page.width() as f64;
        let ph = page.height() as f64;

        // Off-screen canvas holding the page at native resolution.
        let Some(src) = page_to_canvas(&document, page) else {
            return;
        };

        // Source sub-rectangle: a page-aspect window scaled by zoom and panned by
        // the normalized offset (mirrors the native `zoom_crop`).
        let zoom = app.preview_zoom.max(1.0) as f64;
        let rw = (pw / zoom).clamp(1.0, pw);
        let rh = (ph / zoom).clamp(1.0, ph);
        let sx = app.preview_off.0 as f64 * (pw - rw);
        let sy = app.preview_off.1 as f64 * (ph - rh);

        // Destination: fit the window into the viewport, preserving aspect.
        let scale = (cw as f64 / rw).min(ch as f64 / rh);
        let dw = rw * scale;
        let dh = rh * scale;
        let dx = (cw as f64 - dw) / 2.0;
        let dy = (ch as f64 - dh) / 2.0;

        ctx.set_image_smoothing_enabled(true);
        let _ = ctx
            .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
                &src, sx, sy, rw, rh, dx, dy, dw, dh,
            );

        LAST.with(|l| *l.borrow_mut() = Some(key));
    }

    /// Hide the overlay canvas (if it exists) and forget the last view.
    pub fn hide() {
        if let Some(canvas) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id(CANVAS_ID))
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
        {
            let _ = canvas.style().set_property("display", "none");
        }
        LAST.with(|l| *l.borrow_mut() = None);
    }

    /// Fetch the overlay canvas, creating and positioning it on first use.
    fn ensure_canvas(document: &web_sys::Document) -> Option<HtmlCanvasElement> {
        if let Some(existing) = document
            .get_element_by_id(CANVAS_ID)
            .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
        {
            return Some(existing);
        }
        let canvas: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
        canvas.set_id(CANVAS_ID);
        let style = canvas.style();
        // Fixed, full-window, above the terminal grid. `pointer-events: none`
        // keeps DOM events flowing to the editor underneath.
        let _ = style.set_property("position", "fixed");
        let _ = style.set_property("left", "0");
        let _ = style.set_property("top", "0");
        let _ = style.set_property("z-index", "1000");
        let _ = style.set_property("pointer-events", "none");
        document.body()?.append_child(&canvas).ok()?;
        Some(canvas)
    }

    /// Copy a rasterized page into a fresh off-screen canvas at native size.
    fn page_to_canvas(
        document: &web_sys::Document,
        page: &image::RgbaImage,
    ) -> Option<HtmlCanvasElement> {
        let (w, h) = (page.width(), page.height());
        let canvas: HtmlCanvasElement = document.create_element("canvas").ok()?.dyn_into().ok()?;
        canvas.set_width(w);
        canvas.set_height(h);
        let ctx = canvas
            .get_context("2d")
            .ok()??
            .dyn_into::<CanvasRenderingContext2d>()
            .ok()?;
        let data = web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(page.as_raw()), w, h)
            .ok()?;
        ctx.put_image_data(&data, 0.0, 0.0).ok()?;
        Some(canvas)
    }
}
