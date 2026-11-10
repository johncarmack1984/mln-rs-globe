//! A spinning globe with an atmosphere and stars: mln-rs, the Rust port of
//! the MapLibre Native core, in a winit window.
//!
//! The map draws the built-in style (style.json: Natural Earth's relief with
//! OpenFreeMap's water and borders, on the globe projection with a sky), or
//! the style given with `--style`, which gets the globe and an atmosphere if
//! it has neither. Drag to turn the globe, scroll to zoom, space pauses the
//! spin, S saves a screenshot, Esc quits.

mod frontend;
mod target;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mln_rs::mln_actor::RunLoop;
use mln_rs::mln_gfx::RendererBackend;
use mln_rs::mln_gfx::renderable::Renderable;
use mln_rs::mln_map::{AnimationOptions, CameraOptions, Map, MapLoadError, MapObserver, MapOptions};
use mln_rs::mln_platform_default_image::encode_png;
use mln_rs::mln_renderer::Renderer;
use mln_rs::mln_storage::{ClientOptions, ResourceOptions};
use mln_rs::mln_style::PropertyValue;
use mln_rs::mln_style::projection::Projection;
use mln_rs::mln_style::sky::Sky;
use mln_rs::mln_style_types::{ProjectionDefinition, ProjectionType};
use mln_rs::mln_util::geo::{LatLng, ScreenCoordinate};
use mln_rs::mln_util::size::Size;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::frontend::Frontend;
use crate::target::WindowTarget;

const STYLE: &str = include_str!("../style.json");

const USAGE: &str = "\
mln-rs-globe: a spinning globe with an atmosphere and stars, drawn by mln-rs

Usage: mln-rs-globe [OPTIONS]

Options:
  --style URL|FILE   Draw this style instead of the built-in one. It gets the
                     globe projection and an atmosphere if it has neither
  --lat DEGREES      Latitude to start over [default: 20]
  --lon DEGREES      Longitude to start over [default: -30]
  --zoom ZOOM        Zoom [default: 1.6]
  --speed DEGREES    How fast the globe turns, in degrees a second; 0 holds it
                     still [default: 6]
  --screenshot FILE  Save a PNG once the map has loaded, then quit
  -h, --help         Print this help

Drag to turn the globe, scroll to zoom, space pauses the spin, S saves a
screenshot, Esc quits.
";

struct Args {
    style: Option<String>,
    lat: f64,
    lon: f64,
    zoom: f64,
    speed: f64,
    screenshot: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { style: None, lat: 20.0, lon: -30.0, zoom: 1.6, speed: 6.0, screenshot: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        // --name value or --name=value
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| it.next()).ok_or_else(|| format!("{name} needs a value"));
        let number = |s: String| s.parse::<f64>().map_err(|_| format!("{name} takes a number, not {s}"));
        match name.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--style" => args.style = Some(value()?),
            "--lat" => args.lat = number(value()?)?,
            "--lon" => args.lon = number(value()?)?,
            "--zoom" => args.zoom = number(value()?)?,
            "--speed" => args.speed = number(value()?)?,
            "--screenshot" => args.screenshot = Some(value()?.into()),
            _ => return Err(format!("Unknown option {arg}")),
        }
    }
    Ok(args)
}

#[derive(Default)]
struct Observer {
    style_loaded: Cell<bool>,
}

impl MapObserver for Observer {
    fn on_did_fail_loading_map(&self, error: MapLoadError, what: &str) {
        eprintln!("The map failed to load ({error:?}): {what}");
    }

    fn on_did_finish_loading_style(&self) {
        self.style_loaded.set(true);
    }
}

/// Sent by the map's RunLoop when it has work, such as a tile that arrived.
struct Wake;

/// The window and the map in it.
struct Globe {
    // Dropped first: the map resets the renderer through the frontend.
    map: Map,
    frontend: Frontend,
    target: Rc<WindowTarget>,
    observer: Rc<Observer>,
    window: Arc<Window>,

    spinning: bool,
    dragging: bool,
    cursor: ScreenCoordinate,
    opened: Instant,
    last_frame: Instant,
    loaded: Option<Instant>,
    save_next_frame: Option<PathBuf>,
}

/// The map's size, in logical pixels, for the window's `size`.
fn map_size(window: &Window, size: PhysicalSize<u32>) -> Size {
    let size = size.to_logical::<f64>(window.scale_factor());
    Size::new(size.width.round() as u32, size.height.round() as u32)
}

impl Globe {
    fn open(event_loop: &ActiveEventLoop, args: &Args) -> Result<Globe, String> {
        let attributes =
            Window::default_attributes().with_title("mln-rs globe").with_inner_size(LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attributes).map_err(|e| e.to_string())?);
        let pixel_ratio = window.scale_factor() as f32;

        let target = Rc::new(WindowTarget::new(event_loop, window.clone())?);
        let backend: Rc<dyn RendererBackend> = target.clone();
        let frontend = Frontend::new(Renderer::new(backend.clone(), pixel_ratio), backend);
        let observer = Rc::new(Observer::default());
        let mut map = Map::with_resource_options(
            &frontend,
            observer.clone(),
            MapOptions::new().with_size(map_size(&window, window.inner_size())).with_pixel_ratio(pixel_ratio),
            &ResourceOptions::default(),
            &ClientOptions::default(),
        )?;
        map.jump_to(
            &CameraOptions::default().with_center(Some(LatLng::new(args.lat, args.lon)?)).with_zoom(Some(args.zoom)),
        );
        match &args.style {
            None => map.get_style_mut().load_json(STYLE),
            Some(url) if url.contains("://") => map.get_style_mut().load_url(url),
            Some(path) => {
                let json = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                map.get_style_mut().load_json(&json);
            }
        }

        let now = Instant::now();
        Ok(Globe {
            map,
            frontend,
            target,
            observer,
            window,
            spinning: args.speed != 0.0,
            dragging: false,
            cursor: ScreenCoordinate::new(0.0, 0.0),
            opened: now,
            last_frame: now,
            loaded: None,
            save_next_frame: None,
        })
    }

    /// One frame: turn the globe, draw the map's latest update and show it
    /// over the stars. False once `--screenshot` saved its picture.
    fn frame(&mut self, args: &Args) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame).as_secs_f64().min(0.1);
        self.last_frame = now;

        if self.observer.style_loaded.take() && args.style.is_some() {
            put_on_globe(&mut self.map);
        }
        if self.spinning && !self.dragging {
            self.spin(args.speed * elapsed);
        }
        self.frontend.render();

        let mut keep_going = true;
        if let Some(path) = &args.screenshot {
            if self.loaded.is_none() && self.map.is_fully_loaded() {
                self.loaded = Some(now);
            }
            // A second for the tiles to fade in; or give up waiting.
            let settled = self.loaded.is_some_and(|loaded| now - loaded > Duration::from_secs(1));
            if settled || now - self.opened > Duration::from_secs(60) {
                if !settled {
                    eprintln!("The map did not finish loading in a minute");
                }
                self.save_next_frame = Some(path.clone());
                keep_going = false;
            }
        }
        if let Some(path) = self.save_next_frame.take() {
            self.save(&path);
        }

        self.target.present();
        self.window.request_redraw();
        keep_going
    }

    /// Turns the globe east by `degrees`, as the Earth turns.
    fn spin(&mut self, degrees: f64) {
        let Some(center) = self.map.get_camera_options(None).center else {
            return;
        };
        let longitude = (center.longitude() - degrees + 180.0).rem_euclid(360.0) - 180.0;
        if let Ok(center) = LatLng::new(center.latitude(), longitude) {
            self.map.jump_to(&CameraOptions::default().with_center(Some(center)));
        }
    }

    fn save(&self, path: &Path) {
        match self.target.screenshot() {
            Ok(image) => match std::fs::write(path, encode_png(&image)) {
                Ok(()) => println!("Saved {}", path.display()),
                Err(e) => eprintln!("{}: {e}", path.display()),
            },
            Err(e) => eprintln!("{e}"),
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        let framebuffer = Size::new(size.width, size.height);
        if framebuffer.width == 0 || framebuffer.height == 0 || framebuffer == self.target.get_size() {
            return;
        }
        self.target.resize(framebuffer);
        if let Err(e) = self.map.set_size(map_size(&self.window, size)) {
            eprintln!("{e}");
        }
    }

    fn move_cursor(&mut self, position: PhysicalPosition<f64>) {
        let position = position.to_logical::<f64>(self.window.scale_factor());
        let cursor = ScreenCoordinate::new(position.x, position.y);
        if self.dragging {
            let offset = ScreenCoordinate::new(cursor.x - self.cursor.x, cursor.y - self.cursor.y);
            if let Err(e) = self.map.move_by_with_anchor(&offset, Some(cursor), &AnimationOptions::default()) {
                eprintln!("{e}");
            }
        }
        self.cursor = cursor;
    }

    fn zoom(&mut self, delta: MouseScrollDelta) {
        // In wheel notches: a trackpad scrolls by pixels.
        let notches = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y),
            MouseScrollDelta::PixelDelta(position) => position.y / self.window.scale_factor() / 40.0,
        };
        let scale = 2f64.powf(notches / 4.0);
        self.map.scale_by(scale, Some(self.cursor), &AnimationOptions::default());
    }
}

/// Gives a style from `--style` the globe projection and an atmosphere,
/// unless it has them already.
fn put_on_globe(map: &mut Map) {
    let mut style = map.get_style_mut();
    let projection = style.get_projection().get_type();
    if projection.is_undefined()
        || (projection.is_constant() && projection.as_constant().from == ProjectionType::Mercator)
    {
        let mut globe = Projection::new();
        globe.set_type(PropertyValue::Constant(ProjectionDefinition::new(ProjectionType::Globe)));
        style.set_projection(globe);
    }
    if style.get_sky().is_none() {
        let mut sky = Sky::new();
        sky.set_atmosphere_blend(PropertyValue::Constant(1.0));
        style.set_sky(Some(sky));
    }
}

fn screenshot_name() -> PathBuf {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or_default();
    PathBuf::from(format!("mln-rs-globe-{seconds}.png"))
}

struct App {
    args: Args,
    run_loop: RunLoop,
    globe: Option<Globe>,
}

impl ApplicationHandler<Wake> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.globe.is_some() {
            return;
        }
        match Globe::open(event_loop, &self.args) {
            Ok(globe) => self.globe = Some(globe),
            Err(e) => {
                eprintln!("{e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _wake: Wake) {
        // The next frame runs the RunLoop.
        if let Some(globe) = &self.globe {
            globe.window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window: WindowId, event: WindowEvent) {
        let Some(globe) = &mut self.globe else {
            return;
        };
        let mut quit = false;
        match event {
            WindowEvent::CloseRequested => quit = true,
            WindowEvent::Resized(size) => globe.resize(size),
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed && !event.repeat => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => quit = true,
                    Key::Named(NamedKey::Space) => globe.spinning = !globe.spinning,
                    Key::Character(c) if c.eq_ignore_ascii_case("s") => globe.save_next_frame = Some(screenshot_name()),
                    _ => {}
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                globe.dragging = state == ElementState::Pressed;
                globe.map.set_gesture_in_progress(globe.dragging);
            }
            WindowEvent::CursorMoved { position, .. } => globe.move_cursor(position),
            WindowEvent::MouseWheel { delta, .. } => globe.zoom(delta),
            WindowEvent::RedrawRequested => {
                self.run_loop.run_once();
                quit = !globe.frame(&self.args);
            }
            _ => {}
        }
        if quit {
            self.globe = None;
            event_loop.exit();
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    // The map's loop, on this thread: the window's event loop runs it once a
    // frame, and its waker wakes the window when it has work in between.
    let run_loop = RunLoop::new();
    let event_loop = match EventLoop::<Wake>::with_user_event().build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let proxy = Mutex::new(event_loop.create_proxy());
    run_loop.set_waker(Some(Arc::new(move || {
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(Wake);
        }
    })));

    if args.screenshot.is_none() {
        println!("Drag to turn the globe, scroll to zoom, space pauses the spin, S saves a screenshot, Esc quits.");
    }
    let mut app = App { args, run_loop, globe: None };
    if let Err(e) = event_loop.run_app(&mut app) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
