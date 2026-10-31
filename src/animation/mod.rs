//! The animation frame clock: presentation-paced scheduling for the timeline driver.
//!
//! `pacing` holds the decisions (backend choice, refresh-derived interval, monotonic frame
//! deltas) as pure code; `clock` holds the three native waits and the async driver the
//! daemon's frame worker uses.

pub mod clock;
pub mod pacing;

pub use clock::FrameClock;
pub use pacing::{Backend, Burst, Frame, Pacer, Wait};
