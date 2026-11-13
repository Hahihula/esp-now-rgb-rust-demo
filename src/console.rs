//! Keyboard input from the serial monitor.
//!
//! `espflash monitor` (and `cargo run-<board>`, which ends in it) sends every key you press to
//! the board. Dev boards reach the PC either through a USB-to-UART bridge chip (UART0) or the
//! chip's own USB-Serial-JTAG port, and some boards have both. We listen on both, so it does not
//! matter which USB connector you plugged in.
//!
//! Each listener echoes what you type and hands finished lines to the chat task.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::{Async, uart::UartRx, usb::usb_serial_jtag::UsbSerialJtagRx};
use esp_println::print;

/// Longer lines are cut; a packet only carries 250 bytes anyway.
pub const LINE_MAX: usize = 256;

pub type Line = heapless::String<LINE_MAX>;

/// Everything that can wake up the chat task besides an incoming packet.
// `Line` is much bigger than `Button`; with a 4-slot queue that costs about 1 KiB, which is fine.
#[allow(clippy::large_enum_variant)]
pub enum Event {
    Line(Line),
    Button,
}

pub static EVENTS: Channel<CriticalSectionRawMutex, Event, 4> = Channel::new();

/// A tiny line editor: echo, backspace, Enter.
#[derive(Default)]
struct LineEditor {
    line: Line,
}

impl LineEditor {
    async fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'\r' | b'\n' => {
                    print!("\r\n");
                    let line = core::mem::take(&mut self.line);
                    // "\r\n" gives an empty second line; the chat task ignores those.
                    EVENTS.send(Event::Line(line)).await;
                }
                0x08 | 0x7F => {
                    if self.line.pop().is_some() {
                        print!("\x08 \x08");
                    }
                }
                0x20..=0x7E => {
                    if self.line.push(char::from(byte)).is_err() {
                        continue; // line is full
                    }
                    print!("{}", char::from(byte));
                }
                // Keep non-ASCII bytes (unechoed) so the chat task can refuse the line, like
                // the original does.
                0x80.. => {
                    let _ = self.line.push(char::from(byte));
                }
                _ => {} // other control characters
            }
        }
    }
}

#[embassy_executor::task]
pub async fn uart_console(mut rx: UartRx<'static, Async>) {
    let mut editor = LineEditor::default();
    let mut buf = [0u8; 64];
    loop {
        match rx.read_async(&mut buf).await {
            Ok(n) => editor.feed(&buf[..n]).await,
            Err(e) => log::debug!("UART RX error: {e:?}"),
        }
    }
}

#[embassy_executor::task]
pub async fn usb_console(mut rx: UsbSerialJtagRx<'static, Async>) {
    let mut editor = LineEditor::default();
    let mut buf = [0u8; 64];
    loop {
        // USB-Serial-JTAG reads cannot fail (the error type is `Infallible`).
        let Ok(n) = embedded_io_async::Read::read(&mut rx, &mut buf).await;
        editor.feed(&buf[..n]).await;
    }
}
