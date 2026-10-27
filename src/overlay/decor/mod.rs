//! The overlay layer: layered, never-activated windows (click-through except on tabs) drawing niri's window
//! decorations (focus ring, border, shadow, tab indicator, insert hint) around windows that
//! belong to other programs.
//!
//! * [`gradient`]: linear-gradient coordinates and interpolation matching niri's shader.
//! * [`paint`]: a small software rasterizer (rounded shapes, rings, gaussian shadows, linear
//!   gradients) into a premultiplied BGRA surface.  Pure, tested pixel by pixel.
//! * [`spec`]: what to draw, as plain data, and how to paint it.  Pure.
//! * `layer`: the windows. Created on first use on the existing hotkey message thread;
//!   no decoration thread or timer, and no work when nothing changes.
//!
//! The engine describes the decorations of a monitor after every layout pass
//! (`layout/engine/decor.rs`) and hands them to a [`DecorSink`]: the real layer in the
//! daemon, a recording one in tests.

pub mod gradient;
pub mod paint;
pub mod spec;

#[cfg(target_os = "windows")]
pub mod layer;

use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

pub use spec::{
    paint_of, BackdropSpec, DecorContent, DecorId, DecorKind, DecorSpec, FrameSpec, HintSpec,
    ShadowSpec, TabsSpec, TRANSIENT_OUTPUT,
};

/// Where the engine (and the hooks) send decorations.
pub trait DecorSink: Send + Sync {
    /// Replace the decorations of `output` (a monitor, or [`TRANSIENT_OUTPUT`]) with `specs`.
    /// Cheap when nothing changed.
    fn set_output(&self, output: u64, specs: Vec<DecorSpec>);

    /// The window `hwnd` moved or changed size without the engine's doing (a drag by its
    /// title bar): the frame around it follows.
    fn moved(&self, hwnd: isize);

    /// The window `hwnd` came to the front: frames go below it and clickable tabs above it.
    fn restack(&self, hwnd: isize);

    /// Remove every decoration and release its bitmap/window resources.
    fn shutdown(&self);
}

static GLOBAL: OnceLock<Arc<dyn DecorSink>> = OnceLock::new();

/// Make `sink` the layer the hooks talk to.  The first call wins.
pub fn install_global(sink: Arc<dyn DecorSink>) {
    let _ = GLOBAL.set(sink);
}

/// The layer the hooks talk to, once the daemon has installed one.
pub fn global() -> Option<Arc<dyn DecorSink>> {
    GLOBAL.get().cloned()
}

/// A sink that remembers what it was told: for tests of the code that describes decorations.
#[derive(Default)]
pub struct RecordingSink {
    inner: Mutex<RecordingState>,
}

#[derive(Default)]
struct RecordingState {
    outputs: std::collections::BTreeMap<u64, Vec<DecorSpec>>,
    calls: usize,
    shut_down: bool,
}

impl RecordingSink {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Everything shown now, every output together.
    pub fn all(&self) -> Vec<DecorSpec> {
        self.inner
            .lock()
            .outputs
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    /// What `output` shows now.
    pub fn of_output(&self, output: u64) -> Vec<DecorSpec> {
        self.inner
            .lock()
            .outputs
            .get(&output)
            .cloned()
            .unwrap_or_default()
    }

    /// How many times `set_output` was called.
    pub fn calls(&self) -> usize {
        self.inner.lock().calls
    }

    pub fn was_shut_down(&self) -> bool {
        self.inner.lock().shut_down
    }
}

impl DecorSink for RecordingSink {
    fn set_output(&self, output: u64, specs: Vec<DecorSpec>) {
        let mut st = self.inner.lock();
        st.calls += 1;
        if specs.is_empty() {
            st.outputs.remove(&output);
        } else {
            st.outputs.insert(output, specs);
        }
    }

    fn moved(&self, _hwnd: isize) {}

    fn restack(&self, _hwnd: isize) {}

    fn shutdown(&self) {
        let mut st = self.inner.lock();
        st.outputs.clear();
        st.shut_down = true;
    }
}
