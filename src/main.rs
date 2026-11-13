//! ESP-NOW chat room with remote RGB LED control.
//!
//! A `no_std` Rust (esp-hal) port of "Assignment 5: ESP-NOW Chat Room" from Jan Beran's
//! ESP-IDF workshop: <https://honza0297.github.io/EN/assignment-5/>.
//!
//! Type in the serial monitor:
//! - `hello`                broadcast a line to every board in the room
//! - `red`, `green`, ...    set everyone else's LED (see `chat::COMMANDS`)
//! - `wolf1f42:hello`       send to one board only (it must have sent something first)
//! - `list`                 print the boards this one has heard
//!
//! Press BOOT to broadcast the next color in `chat::BUTTON_CYCLE`.

#![no_std]
#![no_main]

extern crate alloc;

mod board;
mod chat;
mod console;
mod led;

use chat::{Contacts, Mac, MacFmt, Name, Remembered, name_str};
use console::{EVENTS, Event};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    efuse::{self, InterfaceMacAddress},
    gpio::{AnyPin, Input, InputConfig, Pull},
    timer::timg::TimerGroup,
    uart::{self, UartRx},
    usb::usb_serial_jtag::UsbSerialJtag,
};
use esp_radio::esp_now::{
    BROADCAST_ADDRESS,
    EspNowManager,
    EspNowSender,
    EspNowWifiInterface,
    PeerInfo,
};
use led::Led;
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

/// Boards using the original ESP-IDF firmware stay on the default Wi-Fi channel, 1.
/// Every board in a room must use the same channel.
const ESP_NOW_CHANNEL: u8 = 1;

#[esp_hal::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));

    // The Wi-Fi driver needs a heap.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    let (led_pin, button_pin, uart_rx_pin) = board::board_pins!(peripherals);

    // ---- RGB LED, off until someone sets it ----
    let mut led = Led::new(peripherals.RMT, led_pin);
    led.set(chat::Rgb::new(0, 0, 0)).await;

    // ---- Keyboard: listen on UART0 and on USB-Serial-JTAG ----
    let uart_rx = UartRx::new(peripherals.UART0, uart::Config::default())
        .expect("UART0 init failed")
        .with_rx(uart_rx_pin)
        .into_async();
    spawner.spawn(console::uart_console(uart_rx).unwrap());

    let (usb_rx, _usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();
    spawner.spawn(console::usb_console(usb_rx).unwrap());

    // ---- BOOT button ----
    spawner.spawn(button(button_pin.into()).unwrap());

    // ---- Wi-Fi radio on (no router), then ESP-NOW ----
    let wifi = esp_radio::wifi::WifiController::new(peripherals.WIFI, Default::default())
        .expect("Wi-Fi init failed");
    // ESP-NOW starts with the broadcast peer (FF:FF:FF:FF:FF:FF) already added.
    let esp_now = wifi.esp_now();
    esp_now
        .set_channel(ESP_NOW_CHANNEL)
        .expect("Setting the Wi-Fi channel failed");
    let (manager, sender, mut receiver) = esp_now.split();

    // ---- Our name in the room ----
    let mac = efuse::interface_mac_address(InterfaceMacAddress::Station);
    let mut mac_bytes: Mac = [0; 6];
    mac_bytes.copy_from_slice(mac.as_bytes());
    let me = chat::node_name(&mac_bytes);

    let mut room = Room {
        me,
        contacts: Contacts::default(),
        manager,
        sender,
        led,
        next_button_color: 0,
    };

    info!(
        "Board: {}, ESP-NOW channel {ESP_NOW_CHANNEL}",
        board::BOARD_NAME
    );
    info!(
        "This board is \"{}\" ({}).",
        name_str(&me),
        MacFmt(&mac_bytes)
    );
    info!("Broadcast a message, or: red, green, blue, white, on, off.");
    info!("Direct message: <8-character name>:<message or command>.");
    info!("Type list to print the names and MAC addresses this board knows.");
    info!("Press BOOT to send everyone the next color.");

    loop {
        match select(receiver.receive_async(), EVENTS.receive()).await {
            Either::First(packet) => {
                room.on_packet(&packet.info.src_address, packet.data())
                    .await;
            }
            Either::Second(Event::Line(line)) => room.on_line(&line).await,
            Either::Second(Event::Button) => room.on_button().await,
        }
    }
}

/// Everything the chat room owns. Only the main loop touches it, so no locking is needed.
struct Room {
    me: Name,
    contacts: Contacts,
    manager: EspNowManager,
    sender: EspNowSender,
    led: Led,
    next_button_color: usize,
}

impl Room {
    /// A line typed in the monitor.
    async fn on_line(&mut self, line: &str) {
        if line.is_empty() {
            return;
        }
        if !chat::is_printable_ascii(line.as_bytes()) {
            warn!("Only ASCII characters are allowed");
            return;
        }
        if line == "list" {
            self.list();
            return;
        }

        let (dest, body) = match chat::split_direct(line) {
            Some((name, body)) => match self.contacts.lookup(&name) {
                Some(mac) => (Some((name, mac)), body),
                None => {
                    warn!(
                        "No contact named {} yet. Wait until that board sends something.",
                        name_str(&name)
                    );
                    return;
                }
            },
            None => (None, line),
        };
        if body.is_empty() {
            return;
        }

        // A color word goes out as its numbers, so the receiver needs no word list.
        let mut encoded: heapless::String<12> = heapless::String::new();
        let text = match chat::find_command(body) {
            Some(c) => {
                use core::fmt::Write;
                let _ = write!(encoded, "{},{},{}", c.r, c.g, c.b);
                encoded.as_str()
            }
            None => body,
        };

        match dest {
            None => {
                if self.send(&BROADCAST_ADDRESS, text).await {
                    info!("Sent: {body}");
                }
            }
            Some((name, mac)) => {
                if self.send(&mac, text).await {
                    info!("Sent to {}: {body}", name_str(&name));
                }
            }
        }
    }

    async fn on_button(&mut self) {
        let word = chat::BUTTON_CYCLE[self.next_button_color];
        self.next_button_color = (self.next_button_color + 1) % chat::BUTTON_CYCLE.len();
        info!("BOOT pressed, sending: {word}");
        self.on_line(word).await;
    }

    /// One ESP-NOW packet from another board.
    async fn on_packet(&mut self, src: &Mac, data: &[u8]) {
        let Some((sender, body)) = chat::decode(data) else {
            if data.len() > chat::NAME_LEN && !chat::is_printable_ascii(data) {
                warn!("Ignored a packet that was not ASCII");
            }
            return;
        };

        if sender == self.me {
            warn!("Another board is using your name");
        } else {
            self.remember(sender, *src);
        }

        if body.is_empty() {
            return;
        }
        match chat::parse_rgb(body) {
            Some(rgb) => {
                self.led.set(rgb).await;
                info!(
                    "User {} set your LED to {},{},{}",
                    name_str(&sender),
                    rgb.r,
                    rgb.g,
                    rgb.b
                );
            }
            None => info!("User {}: {body}", name_str(&sender)),
        }
    }

    /// Keep the contact book and the ESP-NOW peer list in step.
    /// A direct send only works to a MAC that is a registered peer.
    fn remember(&mut self, name: Name, mac: Mac) {
        match self.contacts.remember(name, mac) {
            Remembered::Known => return,
            Remembered::Full => {
                warn!("Contact book is full, not saving {}", name_str(&name));
                return;
            }
            Remembered::Moved { old_mac } => {
                let _ = self.manager.remove_peer(&old_mac);
            }
            Remembered::New => {}
        }

        let peer = PeerInfo {
            interface: EspNowWifiInterface::Station,
            peer_address: mac,
            lmk: None,
            channel: None, // the current channel
            encrypt: false,
        };
        if self.manager.peer_exists(&mac) || self.manager.add_peer(peer).is_ok() {
            info!("Saved contact {} ({})", name_str(&name), MacFmt(&mac));
        } else {
            warn!("Could not add an ESP-NOW peer for {}", name_str(&name));
            self.contacts.forget(&name);
        }
    }

    fn list(&self) {
        if self.contacts.len() == 0 {
            info!("No contacts yet. Wait until another board sends something.");
            return;
        }
        info!("Known contacts ({}):", self.contacts.len());
        for c in self.contacts.iter() {
            info!("  {}  {}", name_str(&c.name), MacFmt(&c.mac));
        }
    }

    /// Returns `true` once the radio has sent the packet.
    ///
    /// For a direct message the result also says whether the other board acknowledged it.
    /// Broadcasts are never acknowledged, so they always count as sent.
    async fn send(&mut self, dest: &Mac, text: &str) -> bool {
        let (packet, trimmed) = chat::encode(&self.me, text);
        if trimmed {
            warn!("Trimming the message to {} bytes", chat::MAX_PACKET_LEN);
        }
        match self.sender.send_async(dest, &packet).await {
            Ok(()) => true,
            Err(e) if *dest != BROADCAST_ADDRESS => {
                warn!(
                    "Send failed, no acknowledgement from {} ({e:?})",
                    MacFmt(dest)
                );
                false
            }
            Err(e) => {
                warn!("Send failed: {e:?}");
                false
            }
        }
    }
}

/// Waits for BOOT presses and tells the chat task. The button pulls the pin low.
#[embassy_executor::task]
async fn button(pin: AnyPin<'static>) {
    let mut button = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
    loop {
        button.wait_for_falling_edge().await;
        EVENTS.send(Event::Button).await;
        // Debounce, then wait for release.
        Timer::after(Duration::from_millis(50)).await;
        button.wait_for_high().await;
        Timer::after(Duration::from_millis(50)).await;
    }
}
