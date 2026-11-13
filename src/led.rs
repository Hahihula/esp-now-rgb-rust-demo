//! A single WS2812 ("NeoPixel") RGB LED driven by the RMT peripheral.
//!
//! The LED reads 24 bits (green, red, blue; most significant bit first). Each bit is one high
//! pulse followed by one low pulse, and only the pulse lengths tell a 0 from a 1. RMT is a
//! peripheral made for exactly that: we hand it a list of `PulseCode`s and it plays them on a
//! pin with exact timing, while the CPU does something else.

use embassy_time::Timer;
use esp_hal::{
    Async,
    gpio::{Level, interconnect::PeripheralOutput},
    peripherals::RMT,
    rmt::{Channel, PulseCode, Rmt, Tx, TxChannelConfig, TxChannelCreator},
    time::Rate,
};

use crate::chat::Rgb;

/// RMT tick frequency. With `clk_divider(1)` one tick is 12.5 ns.
const RMT_FREQ_MHZ: u32 = 80;

// WS2812 timing in ticks of 12.5 ns (datasheet: T0H 0.4 us, T0L 0.85 us, T1H 0.8 us, T1L 0.45 us).
const T0H: u16 = 32;
const T0L: u16 = 68;
const T1H: u16 = 64;
const T1L: u16 = 36;

/// Reset (latch) time in microseconds.
const RESET_US: u64 = 300;

pub struct Led {
    channel: Channel<'static, Async, Tx>,
}

impl Led {
    pub fn new(rmt: RMT<'static>, pin: impl PeripheralOutput<'static>) -> Self {
        let rmt = Rmt::new(rmt, Rate::from_mhz(RMT_FREQ_MHZ))
            .expect("RMT init failed")
            .into_async();
        let channel = rmt
            .channel0
            .configure_tx(
                &TxChannelConfig::default()
                    .with_clk_divider(1)
                    .with_idle_output(true)
                    .with_idle_output_level(Level::Low),
            )
            .expect("RMT channel config failed")
            .with_pin(pin);
        Self { channel }
    }

    pub async fn set(&mut self, color: Rgb) {
        let zero = PulseCode::new(Level::High, T0H, Level::Low, T0L);
        let one = PulseCode::new(Level::High, T1H, Level::Low, T1L);

        // 24 data bits plus the end marker. The LED latches the color once the line stays low.
        let mut pulses = [PulseCode::end_marker(); 25];
        let grb = u32::from(color.g) << 16 | u32::from(color.r) << 8 | u32::from(color.b);
        for (i, pulse) in pulses.iter_mut().take(24).enumerate() {
            *pulse = if grb & (1 << (23 - i)) != 0 {
                one
            } else {
                zero
            };
        }

        if let Err(e) = self.channel.transmit(&pulses).await {
            log::warn!("LED update failed: {e:?}");
        }
        // The LED shows the new color only after the line has been low for the reset time.
        // Wait it out here, so a quick second update cannot run into this one. Newer WS2812B
        // parts need 280 us, older ones 50 us.
        Timer::after_micros(RESET_US).await;
    }
}
