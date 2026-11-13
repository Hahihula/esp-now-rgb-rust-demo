//! Board pin map.
//!
//! Exactly one board feature is enabled, normally by a cargo alias such as `cargo run-c6`.
//! Adding a new board means: a feature in `Cargo.toml`, an alias in `.cargo/config.toml`
//! and one `cfg` block here.

#[cfg(not(any(
    feature = "s31-function-coreboard",
    feature = "s31-korvo",
    feature = "c6-devkitc",
    feature = "c5-devkitc",
    feature = "c3-rust-board",
    feature = "c3-lcdkit",
)))]
compile_error!(
    "Select a board, for example `cargo run-c6`. See the [alias] table in .cargo/config.toml."
);

/// Takes the board's pins out of `esp_hal::Peripherals`:
/// `(RGB LED data pin, BOOT button pin, UART0 RX pin)`.
///
/// It is a macro rather than a function because it moves single fields out of the
/// peripherals struct; the rest stays usable in `main`.
#[cfg(feature = "s31-function-coreboard")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO60, $p.GPIO61, $p.GPIO59)
    };
}
#[cfg(feature = "s31-function-coreboard")]
pub const BOARD_NAME: &str = "ESP32-S31-Function-CoreBoard-1";

#[cfg(feature = "s31-korvo")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO37, $p.GPIO61, $p.GPIO59)
    };
}
#[cfg(feature = "s31-korvo")]
pub const BOARD_NAME: &str = "ESP32-S31-Korvo-1";

#[cfg(feature = "c6-devkitc")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO8, $p.GPIO9, $p.GPIO17)
    };
}
#[cfg(feature = "c6-devkitc")]
pub const BOARD_NAME: &str = "ESP32-C6-DevKitC-1";

#[cfg(feature = "c5-devkitc")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO27, $p.GPIO28, $p.GPIO12)
    };
}
#[cfg(feature = "c5-devkitc")]
pub const BOARD_NAME: &str = "ESP32-C5-DevKitC-1";

#[cfg(feature = "c3-rust-board")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO2, $p.GPIO9, $p.GPIO20)
    };
}
#[cfg(feature = "c3-rust-board")]
pub const BOARD_NAME: &str = "ESP32-C3-DevKit-RUST-1";

// The rotary encoder's push switch is wired to GPIO9, the ESP32-C3 BOOT strapping pin.
#[cfg(feature = "c3-lcdkit")]
macro_rules! board_pins {
    ($p:ident) => {
        ($p.GPIO8, $p.GPIO9, $p.GPIO20)
    };
}
#[cfg(feature = "c3-lcdkit")]
pub const BOARD_NAME: &str = "ESP32-C3-LCDkit";

pub(crate) use board_pins;
