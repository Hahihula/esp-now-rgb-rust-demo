# ESP-NOW RGB chat room in `no_std` Rust

A room full of ESP32 boards talking to each other directly over ESP-NOW (no router, no IP). Type a
line in the serial monitor and every other board prints it. Type a color and every other board's
RGB LED changes. Press BOOT to send the next color.

Written in `no_std` Rust with [esp-hal](https://github.com/esp-rs/esp-hal),
[esp-radio](https://github.com/esp-rs/esp-hal/tree/main/esp-radio) and
[Embassy](https://embassy.dev).

## Acknowledgement

This demo is a Rust port of **"Assignment 5: ESP-NOW Chat Room"** from **Jan Beran**'s ESP-IDF and
ESP32-C6 workshop: <https://honza0297.github.io/EN/assignment-5/>. The idea, the commands, the
board names, and the wire format all come from Jan's assignment. Thank you, Jan!

The original assignment also explains what ESP-NOW is and why it works the way it does. Read it
first; this README only covers what is different in Rust.

The wire format is unchanged, so **boards running this firmware and boards running Jan's C
firmware can share one room**.

## Supported boards

| Board | Chip | Rust target | RGB LED | BOOT button | Command |
|---|---|---|---|---|---|
| [ESP32-S31-Function-CoreBoard-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-function-coreboard-1/user_guide.html) | ESP32-S31 | `riscv32imafc-unknown-none-elf` | GPIO60 | GPIO61 | `cargo run-s31` |
| [ESP32-S31-Korvo-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32s31/esp32-s31-korvo-1/user_guide.html) | ESP32-S31 | `riscv32imafc-unknown-none-elf` | GPIO37 | GPIO61 | `cargo run-korvo` |
| [ESP32-C6-DevKitC-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32c6/esp32-c6-devkitc-1/user_guide.html) | ESP32-C6 | `riscv32imac-unknown-none-elf` | GPIO8 | GPIO9 | `cargo run-c6` |
| [ESP32-C5-DevKitC-1](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32c5/esp32-c5-devkitc-1/user_guide.html) | ESP32-C5 | `riscv32imac-unknown-none-elf` | GPIO27 | GPIO28 | `cargo run-c5` |
| [ESP32-C3-DevKit-RUST-1](https://github.com/esp-rs/esp-rust-board) | ESP32-C3 | `riscv32imc-unknown-none-elf` | GPIO2 | GPIO9 | `cargo run-c3` |
| [ESP32-C3-LCDkit](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32c3/esp32-c3-lcdkit/user_guide.html) (MB board) | ESP32-C3 | `riscv32imc-unknown-none-elf` | GPIO8 | GPIO9 (encoder switch) | `cargo run-c3-lcdkit` |

Every `run-*` alias has a matching `build-*` alias that only compiles.

On the ESP32-C3-LCDkit there is no separate BOOT button: press the rotary encoder knob. It is wired
to GPIO9, the BOOT strapping pin.

## Setup

1. Install Rust with [rustup](https://rustup.rs). The pinned nightly toolchain, `rust-src` and the
   RISC-V targets in [`rust-toolchain.toml`](rust-toolchain.toml) install themselves on the first
   build.
2. Install espflash, which flashes the board and opens the serial monitor:

   ```bash
   cargo install espflash --locked
   ```

3. Plug in the board and run the alias for it, for example:

   ```bash
   cargo run-c6
   ```

   This builds, flashes, and opens the monitor. Exit the monitor with `Ctrl-C`.

The aliases live in [`.cargo/config.toml`](.cargo/config.toml). Each one picks the Rust target and
the board feature, and `espflash flash --monitor` is the runner.

If you prefer the `cargo espflash` subcommand (`cargo install cargo-espflash --locked`), pass the
same target and feature yourself:

```bash
cargo espflash flash --release --monitor --target riscv32imac-unknown-none-elf --features c6-devkitc
```

ESP32-S31 is not in a crates.io release of esp-hal yet, so `Cargo.toml` pins the esp-hal crates to
a git revision.

## Using the chat room

Each board names itself from its MAC address: a 4-letter animal plus the last two MAC bytes in hex,
for example `bear83e8`. Look for `This board is "..."` in the monitor right after boot.

| You type | What happens |
|---|---|
| `hello` | Every other board prints `User bear83e8: hello`. |
| `red`, `green`, `blue`, `white`, `on`, `off` | Every other board's LED changes and prints `User bear83e8 set your LED to 20,0,0`. |
| `wolf1f42:hello` or `wolf1f42:blue` | Only `wolf1f42` gets it. That board must have sent something first. |
| `list` | Prints the boards this one has heard, with their MAC addresses. Nothing is sent. |
| BOOT button | Sends the next color: red, green, blue, white, off, and around again. |

Rules carried over from the original:

- Only printable ASCII is sent. Lines with other characters are refused.
- A packet longer than 250 bytes is cut to fit.
- A board does not receive its own broadcast.
- If another board uses your name, you get a warning with its MAC address. Unlike the
  original, the message itself is not shown, so it cannot look like it came from you.
- The room remembers 19 names: ESP-NOW keeps 20 peers and one is the broadcast address.

Direct messages are acknowledged by the receiving radio. If nobody acknowledges, the monitor says
`Send failed, no acknowledgement`. Broadcasts are never acknowledged.

All boards in one room must be on the same Wi-Fi channel. This demo uses channel 1, the channel
an ESP-IDF station sits on before it joins a router, so it matches the C version. Change
`ESP_NOW_CHANNEL` in `src/main.rs` to give your table its own room.

### Wire format

```text
bear83e8:hello      chat text from bear83e8
bear83e8:20,0,0     bear83e8 sets your LED to r=20, g=0, b=0
```

The sender's 8-character name, a colon, then the body. A body that is exactly three numbers from 0
to 255 separated by commas is a color. Anything else is chat text. Color words never go over the
air, only their numbers, so a new color word only has to be known by the board that sends it.

## How the code is organised

| File | What it does |
|---|---|
| [`src/main.rs`](src/main.rs) | Start-up, and the chat task: one loop that waits for a packet, a typed line, or a button press. |
| [`src/chat.rs`](src/chat.rs) | The protocol and the contact book: names, color words, parsing typed lines (`parse_line`), `r,g,b` parsing, encode and decode. No hardware. |
| [`src/console.rs`](src/console.rs) | Reads keys from the monitor, echoes them, and hands finished lines to the chat task. |
| [`src/led.rs`](src/led.rs) | Drives the WS2812 RGB LED with the RMT peripheral. |
| [`src/board.rs`](src/board.rs) | Pin numbers for each board. |

Compared with the C version:

- **Tasks instead of FreeRTOS threads.** The ESP-IDF version has a receive callback, a queue, an
  RX task, an input task, and a mutex around the contact book. Here one Embassy task owns ESP-NOW
  and the contact book and `select`s over incoming packets and an event channel, so it needs no
  lock.
- **Both serial ports.** Some boards reach the PC through a USB-to-UART bridge (UART0), others
  through the chip's own USB-Serial-JTAG, and some have both connectors. The firmware reads keys
  from both, so either connector works. Output goes to USB-Serial-JTAG once a USB host has been
  seen on it, otherwise to UART0 (esp-println's `auto` mode). So if both cables are plugged in,
  open the monitor on the USB-Serial-JTAG port.
- **No `led_strip` component.** `led.rs` turns the 24 color bits into RMT pulses itself. It is about
  30 lines and a good place to see how RMT works.
- **No NVS.** esp-radio does not need it to start the Wi-Fi radio.

## Adding a board

1. Add a feature in `Cargo.toml` that enables the chip feature, for example
   `my-board = ["esp32c6"]`.
2. Add a `cfg` block with the LED, button and UART0 RX pins in `src/board.rs`.
3. Add `build-my-board` and `run-my-board` aliases in `.cargo/config.toml`.
