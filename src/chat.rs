//! The chat room protocol and the contact book. No hardware in here.
//!
//! The wire format is the same as in the original ESP-IDF chat room, so Rust and C boards can
//! share one room:
//!
//! ```text
//! bear83e8:hello      <- chat text from bear83e8
//! bear83e8:20,0,0     <- bear83e8 sets your LED to r=20 g=0 b=0
//! ```
//!
//! Every packet is plain ASCII: the sender's 8-character name, `:`, then the body.
//! Color words such as `red` never go over the air, only their numbers do.

use core::fmt::Write;

/// Every board name is exactly this long.
pub const NAME_LEN: usize = 8;

/// ESP-NOW v1 payload limit (`ESP_NOW_MAX_DATA_LEN`).
pub const MAX_PACKET_LEN: usize = 250;

/// ESP-NOW keeps 20 peers and one is the broadcast address.
pub const MAX_CONTACTS: usize = 19;

pub type Name = [u8; NAME_LEN];
pub type Mac = [u8; 6];
pub type Packet = heapless::Vec<u8, MAX_PACKET_LEN>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Add a row to teach the room a new color word. Other boards only ever see the numbers.
pub const COMMANDS: &[(&str, Rgb)] = &[
    ("red", Rgb::new(20, 0, 0)),
    ("green", Rgb::new(0, 20, 0)),
    ("blue", Rgb::new(0, 0, 20)),
    ("white", Rgb::new(20, 20, 20)),
    ("on", Rgb::new(20, 20, 20)),
    ("off", Rgb::new(0, 0, 0)),
];

/// Colors the BOOT button cycles through.
pub const BUTTON_CYCLE: &[&str] = &["red", "green", "blue", "white", "off"];

pub fn find_command(word: &str) -> Option<Rgb> {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, rgb)| *rgb)
}

/// Printable ASCII only (0x20..=0x7E), like the original.
pub fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| (0x20..=0x7E).contains(b))
}

const ANIMALS: [&str; 16] = [
    "bear", "wolf", "lynx", "seal", "crab", "frog", "duck", "goat", "hawk", "mole", "newt", "swan",
    "toad", "moth", "deer", "orca",
];

/// A 4-letter animal picked from the MAC, then the last two MAC bytes in hex: `bear83e8`.
/// Same recipe as the C version, so a board keeps its name whichever firmware it runs.
pub fn node_name(mac: &Mac) -> Name {
    let animal = ANIMALS[usize::from(mac[3] ^ mac[4] ^ mac[5]) % ANIMALS.len()];
    let mut s: heapless::String<NAME_LEN> = heapless::String::new();
    // Cannot fail: 4 + 2 * 2 characters.
    let _ = write!(s, "{animal}{:02x}{:02x}", mac[4], mac[5]);
    let mut name = [0; NAME_LEN];
    name.copy_from_slice(s.as_bytes());
    name
}

pub fn name_str(name: &Name) -> &str {
    // Names are only ever built from printable ASCII.
    core::str::from_utf8(name).unwrap_or("????????")
}

/// `"20,0,0"` is a color. Anything else, including extra characters or spaces, is chat text.
pub fn parse_rgb(s: &str) -> Option<Rgb> {
    let mut parts = s.split(',');
    let mut next = || -> Option<u8> {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let rgb = Rgb::new(next()?, next()?, next()?);
    if parts.next().is_some() {
        return None;
    }
    Some(rgb)
}

/// `"<name>:<body>"`, cut to fit one ESP-NOW packet.
pub fn encode(from: &Name, body: &str) -> (Packet, bool) {
    let mut packet = Packet::new();
    let _ = packet.extend_from_slice(from);
    let _ = packet.push(b':');
    let room = MAX_PACKET_LEN - packet.len();
    let trimmed = body.len() > room;
    let _ = packet.extend_from_slice(&body.as_bytes()[..body.len().min(room)]);
    (packet, trimmed)
}

/// Splits a received packet into sender and body. `None` if it is not a chat packet.
pub fn decode(data: &[u8]) -> Option<(Name, &str)> {
    if data.len() < NAME_LEN + 1 || data[NAME_LEN] != b':' || !is_printable_ascii(data) {
        return None;
    }
    let mut name = [0; NAME_LEN];
    name.copy_from_slice(&data[..NAME_LEN]);
    let body = core::str::from_utf8(&data[NAME_LEN + 1..]).ok()?;
    Some((name, body))
}

/// A typed line that starts with `<8-character name>:` is a direct message.
pub fn split_direct(line: &str) -> Option<(Name, &str)> {
    let bytes = line.as_bytes();
    if bytes.len() < NAME_LEN + 1 || bytes[NAME_LEN] != b':' {
        return None;
    }
    let mut name = [0; NAME_LEN];
    name.copy_from_slice(&bytes[..NAME_LEN]);
    Some((name, &line[NAME_LEN + 1..]))
}

#[derive(Clone, Copy)]
pub struct Contact {
    pub name: Name,
    pub mac: Mac,
}

/// What `Contacts::remember` changed, so the caller can update the ESP-NOW peer list.
pub enum Remembered {
    /// Seen before with the same MAC. Nothing to do.
    Known,
    /// A new board. Add its MAC as a peer.
    New,
    /// A known name came from a different MAC. Replace the old peer with the new one.
    Moved { old_mac: Mac },
    /// The book is full.
    Full,
}

/// Names this board has heard, with the MAC each one came from.
#[derive(Default)]
pub struct Contacts {
    entries: heapless::Vec<Contact, MAX_CONTACTS>,
}

impl Contacts {
    pub fn lookup(&self, name: &Name) -> Option<Mac> {
        self.entries.iter().find(|c| &c.name == name).map(|c| c.mac)
    }

    pub fn remember(&mut self, name: Name, mac: Mac) -> Remembered {
        if let Some(contact) = self.entries.iter_mut().find(|c| c.name == name) {
            if contact.mac == mac {
                return Remembered::Known;
            }
            let old_mac = core::mem::replace(&mut contact.mac, mac);
            return Remembered::Moved { old_mac };
        }
        match self.entries.push(Contact { name, mac }) {
            Ok(()) => Remembered::New,
            Err(_) => Remembered::Full,
        }
    }

    /// Undo a `New` when the ESP-NOW peer could not be added.
    pub fn forget(&mut self, name: &Name) {
        self.entries.retain(|c| &c.name != name);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

pub struct MacFmt<'a>(pub &'a Mac);

impl core::fmt::Display for MacFmt<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let m = self.0;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            m[0], m[1], m[2], m[3], m[4], m[5]
        )
    }
}
