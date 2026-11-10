//! The map's renderer frontend, as in mln-rs's GLFW app: it keeps the
//! renderer and the map's latest update until the next frame draws it.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use mln_rs::mln_gfx::{BackendScope, RendererBackend, ScopeType};
use mln_rs::mln_renderer::{Renderer, RendererFrontend, RendererObserver, UpdateParameters};

struct State {
    backend: Rc<dyn RendererBackend>,
    renderer: RefCell<Option<Renderer>>,
    update: RefCell<Option<Arc<UpdateParameters>>>,
    dirty: Cell<bool>,
}

/// A handle: the map keeps a clone.
#[derive(Clone)]
pub struct Frontend(Rc<State>);

impl Frontend {
    pub fn new(renderer: Renderer, backend: Rc<dyn RendererBackend>) -> Frontend {
        Frontend(Rc::new(State {
            backend,
            renderer: RefCell::new(Some(renderer)),
            update: RefCell::new(None),
            dirty: Cell::new(false),
        }))
    }

    /// Draws the map's latest update, unless it is drawn already.
    pub fn render(&self) {
        if !self.0.dirty.replace(false) {
            return;
        }
        // Rendering may bring a newer update; draw this one.
        let Some(update) = self.0.update.borrow().clone() else {
            return;
        };
        let _scope = BackendScope::new(&*self.0.backend, ScopeType::Implicit);
        if let Some(renderer) = self.0.renderer.borrow_mut().as_mut() {
            renderer.render(&update);
        }
    }
}

impl RendererFrontend for Frontend {
    fn reset(&self) {
        let renderer = self.0.renderer.borrow_mut().take();
        drop(renderer);
    }

    fn set_observer(&self, observer: Weak<dyn RendererObserver>) {
        if let Some(renderer) = self.0.renderer.borrow_mut().as_mut() {
            renderer.set_observer(Some(observer));
        }
    }

    fn update(&self, update: Arc<UpdateParameters>) {
        *self.0.update.borrow_mut() = Some(update);
        self.0.dirty.set(true);
    }
}
