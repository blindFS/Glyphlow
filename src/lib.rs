//! Glyphlow labels the UI elements of the focused window with hint keys, so
//! they can be picked from the keyboard.
//!
//! `glyphlow` is the server: it owns the overlay, the event tap and the state
//! machine in [`AppEngine`]. `glyphlow-cli` is a thin client that sends it
//! [`AppSignal`]s over a Unix socket ([`ipc`]).

pub mod action;
mod app_engine;
pub mod ax_element;
pub mod config;
pub mod ipc;
mod key_listener;
pub mod os_util;
pub mod user_interface;
pub mod util;

pub use app_engine::*;
pub use key_listener::*;
