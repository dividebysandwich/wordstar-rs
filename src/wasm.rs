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
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    console_error_panic_hook::set_once();

    let app = Rc::new(RefCell::new(
        App::new(None).map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?,
    ));

    let backend = DomBackend::new().map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;
    let mut terminal =
        Terminal::new(backend).map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;

    // Keyboard: translate Ratzilla's web key event into the editor's vocabulary.
    terminal
        .on_key_event({
            let app = app.clone();
            move |ev| {
                app.borrow_mut().handle_key(ev.into());
            }
        })
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;

    // Mouse: clicks/drags map to selection; scroll/enter/exit are ignored.
    terminal
        .on_mouse_event({
            let app = app.clone();
            move |ev| {
                if let Some(me) = crate::input::mouse_event_from(ev) {
                    app.borrow_mut().handle_mouse(me);
                }
            }
        })
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e.to_string()))?;

    // Render loop: advance async work, draw the TUI, then paint the preview
    // canvas on top (or hide it).
    terminal.draw_web(move |frame| {
        let mut app = app.borrow_mut();
        if app.preview_loading() {
            app.step_preview_job();
        }
        app.poll_pending_open();
        ui::draw(frame, &app);
        canvas::render(&app);
    });

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
