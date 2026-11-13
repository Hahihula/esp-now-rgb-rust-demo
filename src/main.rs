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

use chat::{Body, Contacts, Mac, MacFmt, Name, Remembered, Typed, name_str};
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

/// The Wi-Fi channel the radio listens and sends on. Every board in a room must use the same one.
/// Boards running the original ESP-IDF firmware stay on the default channel, 1.
///
/// `set_channel` below calls `esp_wifi_set_channel`, which tunes the radio itself, so broadcasts
/// and direct messages (peers added with `channel: None`) all use this channel.
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
    // `esp_now` holds its own reference to the Wi-Fi driver, so the radio stays on as long as
    // the ESP-NOW handles live, even if `wifi` is dropped.
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
        // `select` polls the first future first. Local events go first: they are rare, so they
        // cannot starve the radio, while a busy room could otherwise delay your typing.
        match select(EVENTS.receive(), receiver.receive_async()).await {
            Either::First(Event::Line(line)) => room.on_line(&line).await,
            Either::First(Event::Button) => room.on_button().await,
            Either::Second(packet) => {
                room.on_packet(&packet.info.src_address, packet.data())
                    .await;
            }
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
    async fn on_line(&mut self, line: &[u8]) {
        match chat::parse_line(line) {
            Typed::Nothing => {}
            Typed::NotAscii => warn!("Only ASCII characters are allowed"),
            Typed::List => self.list(),
            Typed::Send {
                to: None,
                body,
                typed,
            } => {
                if self.send(&BROADCAST_ADDRESS, body).await {
                    info!("Sent: {typed}");
                }
            }
            Typed::Send {
                to: Some(name),
                body,
                typed,
            } => {
                let Some(mac) = self.contacts.lookup(&name) else {
                    warn!(
                        "No contact named {} yet. Wait until that board sends something.",
                        name_str(&name)
                    );
                    return;
                };
                if self.send(&mac, body).await {
                    info!("Sent to {}: {typed}", name_str(&name));
                }
            }
        }
    }

    /// BOOT: broadcast the next color in `chat::BUTTON_CYCLE`.
    async fn on_button(&mut self) {
        let word = chat::BUTTON_CYCLE[self.next_button_color];
        self.next_button_color = (self.next_button_color + 1) % chat::BUTTON_CYCLE.len();
        let rgb = chat::find_command(word).expect("BUTTON_CYCLE words are in COMMANDS");
        if self.send(&BROADCAST_ADDRESS, Body::Color(rgb)).await {
            info!("BOOT pressed, sent: {word}");
        }
    }

    /// One ESP-NOW packet from another board.
    async fn on_packet(&mut self, src: &Mac, data: &[u8]) {
        let Some((sender, body)) = chat::decode(data) else {
            if data.len() > chat::NAME_LEN && !chat::is_printable_ascii(data) {
                warn!("Ignored a packet that was not ASCII");
            }
            return;
        };

        // Our own broadcasts never come back to us, so this is a different board. Show nothing
        // of what it sent: printing "User <our name>: ..." would only add to the confusion.
        if sender == self.me {
            warn!(
                "Another board ({}) is using your name {}; ignoring its message",
                MacFmt(src),
                name_str(&sender)
            );
            return;
        }
        self.remember(sender, *src);

        if body.is_empty() {
            return;
        }
        match chat::parse_rgb(body) {
            Some(rgb) => {
                self.led.set(rgb).await;
                info!(
                    "User {} set your LED to {}",
                    name_str(&sender),
                    chat::encode_color(rgb)
                );
            }
            None => info!("User {}: {body}", name_str(&sender)),
        }
    }

    /// Keep the contact book and the ESP-NOW peer list in step.
    /// A direct send only works to a MAC that is a registered peer.
    fn remember(&mut self, name: Name, mac: Mac) {
        let old_mac = match self.contacts.remember(name, mac) {
            Remembered::Known => return,
            Remembered::Full => {
                warn!("Contact book is full, not saving {}", name_str(&name));
                return;
            }
            Remembered::New => None,
            Remembered::Moved { old_mac } => Some(old_mac),
        };

        // Add the new peer first, so a failure leaves the old, working state in place.
        let peer = PeerInfo {
            interface: EspNowWifiInterface::Station,
            peer_address: mac,
            lmk: None,
            channel: None, // the radio's channel, see ESP_NOW_CHANNEL
            encrypt: false,
        };
        if !self.manager.peer_exists(&mac)
            && let Err(e) = self.manager.add_peer(peer)
        {
            warn!(
                "Could not add an ESP-NOW peer for {} ({e:?})",
                name_str(&name)
            );
            match old_mac {
                Some(old_mac) => {
                    self.contacts.remember(name, old_mac);
                }
                None => self.contacts.forget(&name),
            }
            return;
        }

        if let Some(old_mac) = old_mac
            && let Err(e) = self.manager.remove_peer(&old_mac)
        {
            warn!(
                "Could not remove the old ESP-NOW peer {} ({e:?})",
                MacFmt(&old_mac)
            );
        }
        info!("Saved contact {} ({})", name_str(&name), MacFmt(&mac));
    }

    fn list(&self) {
        if self.contacts.is_empty() {
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
    async fn send(&mut self, dest: &Mac, body: Body<'_>) -> bool {
        let (packet, trimmed) = chat::encode(&self.me, body);
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
        // Let the contacts settle, then check the button is really held. A bounce or a glitch
        // that is already over does not count.
        Timer::after(Duration::from_millis(20)).await;
        if button.is_high() {
            continue;
        }
        EVENTS.send(Event::Button).await;
        // One press, one event: wait for release, and let the release bounce settle too.
        button.wait_for_high().await;
        Timer::after(Duration::from_millis(50)).await;
    }
}
