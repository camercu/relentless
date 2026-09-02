//! Facade module centralizing conditional imports.
//!
//! All other modules import from this facade rather than from `core`/`alloc`/`std`
//! directly. This keeps `#[cfg]` attributes out of the main logic.

pub(crate) use core::time::Duration;

#[cfg(feature = "alloc")]
#[allow(unused_imports)]
pub(crate) use alloc::boxed::Box;
#[cfg(feature = "alloc")]
#[allow(unused_imports)]
pub(crate) use alloc::string::String;
#[cfg(feature = "alloc")]
#[allow(unused_imports)]
pub(crate) use alloc::vec::Vec;
