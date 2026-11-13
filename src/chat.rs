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
    write!(s, "{animal}{:02x}{:02x}", mac[4], mac[5]).expect("a name is 4 + 2 * 2 characters");
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

/// What goes after `<name>:` in a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Body<'a> {
    Text(&'a str),
    /// Sent as its numbers, so the receiver needs no word list.
    Color(Rgb),
}

/// `"20,0,0"`: at most 3 digits per channel, so it always fits.
pub type ColorText = heapless::String<11>;

pub fn encode_color(rgb: Rgb) -> ColorText {
    let mut s = ColorText::new();
    write!(s, "{},{},{}", rgb.r, rgb.g, rgb.b).expect("3 * 3 digits + 2 commas");
    s
}

/// `"<name>:<body>"`, cut to fit one ESP-NOW packet. The flag says whether it was cut.
pub fn encode(from: &Name, body: Body<'_>) -> (Packet, bool) {
    let color;
    let text = match body {
        Body::Text(text) => text.as_bytes(),
        Body::Color(rgb) => {
            color = encode_color(rgb);
            color.as_bytes()
        }
    };

    let mut packet = Packet::new();
    // The name and ':' are 9 bytes; the text is cut to the space left, so nothing overflows.
    let room = MAX_PACKET_LEN - (NAME_LEN + 1);
    let len = text.len().min(room);
    packet.extend_from_slice(from).unwrap();
    packet.push(b':').unwrap();
    packet.extend_from_slice(&text[..len]).unwrap();
    (packet, len < text.len())
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

/// What a line typed in the monitor asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Typed<'a> {
    /// Empty line, or a direct message with nothing after the colon.
    Nothing,
    /// The line has bytes outside printable ASCII.
    NotAscii,
    /// `list`: print the contact book. Nothing is sent.
    List,
    /// Send `body`, to everyone (`to: None`) or to one contact. `typed` is the body as typed,
    /// for the "Sent: ..." log line.
    Send {
        to: Option<Name>,
        body: Body<'a>,
        typed: &'a str,
    },
}

/// Turns a typed line into an [`Typed`]. All command words live here or in [`COMMANDS`].
pub fn parse_line(line: &[u8]) -> Typed<'_> {
    if line.is_empty() {
        return Typed::Nothing;
    }
    if !is_printable_ascii(line) {
        return Typed::NotAscii;
    }
    // Printable ASCII is valid UTF-8.
    let Ok(line) = core::str::from_utf8(line) else {
        return Typed::NotAscii;
    };
    if line == "list" {
        return Typed::List;
    }

    // `<8-character name>:<body>` is a direct message.
    let (to, typed) = match line.as_bytes().get(NAME_LEN) {
        Some(b':') => {
            let mut name = [0; NAME_LEN];
            name.copy_from_slice(&line.as_bytes()[..NAME_LEN]);
            (Some(name), &line[NAME_LEN + 1..])
        }
        _ => (None, line),
    };
    if typed.is_empty() {
        return Typed::Nothing;
    }

    let body = match find_command(typed) {
        Some(rgb) => Body::Color(rgb),
        None => Body::Text(typed),
    };
    Typed::Send { to, body, typed }
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

    /// Undo a `New` when the ESP-NOW peer could not be added. (Undo a `Moved` by remembering
    /// the old MAC again.)
    pub fn forget(&mut self, name: &Name) {
        self.entries.retain(|c| &c.name != name);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
