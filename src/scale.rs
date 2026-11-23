//! Display scale detection from X11 properties. No Wayland dependency.
//!
//! X11 clients under XWayland learn the scale factor through XSettings
//! (`Gdk/WindowScalingFactor`) and X resources (`Xft.dpi`). This module reads
//! both over the existing X11 connection and turns them into a DPI value.

use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, Atom, ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// DPI used when no scale information is available.
pub const BASE_DPI: f32 = 96.0;

/// A name/value pair from an XSettings blob.
#[derive(Debug, PartialEq)]
struct Setting {
    name: Vec<u8>,
    int_value: i32,
}

/// Parses an XSettings `_XSETTINGS_SETTINGS` blob into integer settings.
/// Non-integer settings are skipped. Byte order is taken from the header.
fn parse_xsettings(data: &[u8]) -> Vec<Setting> {
    if data.len() < 12 {
        return Vec::new();
    }
    let little = match data[0] {
        0 => true,
        1 => false,
        _ => return Vec::new(),
    };
    let count = read_u32(&data[8..12], little) as usize;
    let mut out = Vec::new();
    let mut at = 12usize;
    for _ in 0..count {
        if at + 8 > data.len() {
            break;
        }
        let kind = data[at];
        let name_len = read_u16(&data[at + 2..at + 4], little) as usize;
        at += 4;
        if at + name_len > data.len() {
            break;
        }
        let name = data[at..at + name_len].to_vec();
        at += name_len;
        at = align4(at);
        let value_start = at;
        // Integer: 4 bytes. String: 4-byte length followed by bytes.
        // Color: 4 bytes. Each value block is padded to a 4-byte boundary.
        let value_len = match kind {
            0 | 2 => 4,
            1 => {
                if at + 4 > data.len() {
                    break;
                }
                let len = read_u32(&data[at..at + 4], little) as usize;
                at += 4 + len;
                at = align4(at);
                at - value_start
            }
            _ => 4,
        };
        if value_start + 4 > data.len() {
            break;
        }
        if kind == 0 {
            out.push(Setting {
                name,
                int_value: read_u32(&data[value_start..value_start + 4], little) as i32,
            });
        }
        at = align4(value_start + value_len);
    }
    out
}

/// Extracts `Xft.dpi` from an X resources (RESOURCE_MANAGER) blob.
fn parse_xresources(data: &[u8]) -> Option<f32> {
    let text = std::str::from_utf8(data).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Xft.dpi:") {
            if let Ok(dpi) = value.trim().parse::<f32>() {
                if dpi > 0.0 {
                    return Some(dpi);
                }
            }
        }
    }
    None
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn read_u16(data: &[u8], little: bool) -> u16 {
    let bytes = [data[0], data[1]];
    if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    }
}

fn read_u32(data: &[u8], little: bool) -> u32 {
    let bytes = [data[0], data[1], data[2], data[3]];
    if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}

/// Reads the X11Libre RandR per-output `DPI` property. 96 means 1x scaling.
/// Returns the first integer DPI found across the active outputs.
fn randr_dpi(connection: &RustConnection, root: Window) -> Option<f32> {
    let dpi_atom = atom(connection, "DPI")?;
    let resources = connection
        .randr_get_screen_resources_current(root)
        .ok()?
        .reply()
        .ok()?;
    let cookies: Vec<_> = resources
        .outputs
        .iter()
        .filter_map(|&output| {
            connection
                .randr_get_output_info(output, resources.config_timestamp)
                .ok()
                .map(|cookie| (output, cookie))
        })
        .collect();
    for (output, cookie) in cookies {
        let Ok(info) = cookie.reply() else {
            continue;
        };
        if info.connection == randr::Connection::CONNECTED {
            let reply = connection
                .get_property(false, output, dpi_atom, xproto::AtomEnum::INTEGER, 0, 1)
                .ok()?
                .reply()
                .ok()?;
            if reply.format == 32 && reply.value.len() >= 4 {
                let little = connection.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST;
                let dpi = read_u32(&reply.value[..4], little) as f32;
                if dpi > 0.0 {
                    return Some(dpi);
                }
            }
        }
    }
    None
}

fn property(connection: &RustConnection, window: Window, atom: Atom) -> Option<Vec<u8>> {
    let reply = connection
        .get_property(false, window, atom, xproto::AtomEnum::ANY, 0, u32::MAX)
        .ok()?
        .reply()
        .ok()?;
    if reply.value.is_empty() {
        None
    } else {
        Some(reply.value)
    }
}

fn atom(connection: &RustConnection, name: &str) -> Option<Atom> {
    connection
        .intern_atom(false, name.as_bytes())
        .ok()?
        .reply()
        .ok()
        .map(|reply| reply.atom)
}

/// Detects the effective DPI from XSettings and X resources.
///
/// Priority: XSettings `Gdk/WindowScalingFactor` scales `Xft/DPI` (or the
/// base DPI if absent). X resources `Xft.dpi` is used when XSettings has no
/// usable DPI. `GDK_SCALE` is a fallback when no X11 property is available.
pub fn detect(connection: &RustConnection, root: Window) -> f32 {
    if let Some(dpi) = randr_dpi(connection, root) {
        return dpi.clamp(48.0, 384.0);
    }

    let settings_cookie = connection.intern_atom(false, b"_XSETTINGS_SETTINGS").ok();
    let owner_cookie = connection.intern_atom(false, b"_XSETTINGS_S0").ok();
    let settings_atom = settings_cookie.and_then(|c| c.reply().ok()).map(|r| r.atom);
    let xsettings_owner_atom = owner_cookie.and_then(|c| c.reply().ok()).map(|r| r.atom);
    let mut settings = Vec::new();
    if let (Some(owner_atom), Some(settings_atom)) = (xsettings_owner_atom, settings_atom) {
        if let Ok(reply) = connection.get_selection_owner(owner_atom) {
            if let Ok(owner_reply) = reply.reply() {
                if owner_reply.owner != Window::from(0u32) {
                    if let Some(data) = property(connection, owner_reply.owner, settings_atom) {
                        settings = parse_xsettings(&data);
                    }
                }
            }
        }
    }

    let xft_from_settings = settings
        .iter()
        .find(|s| s.name == b"Xft/DPI")
        .and_then(|s| (s.int_value > 0).then_some(s.int_value as f32 / 1024.0));
    let scale_from_settings = settings
        .iter()
        .find(|s| s.name == b"Gdk/WindowScalingFactor")
        .and_then(|s| (s.int_value > 0).then_some(s.int_value as f32));

    let resource_atom = xproto::AtomEnum::RESOURCE_MANAGER.into();
    let dpi = xft_from_settings.or_else(|| {
        property(connection, root, resource_atom).and_then(|data| parse_xresources(&data))
    });

    let env_scale = std::env::var("GDK_SCALE")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| *value > 0.0);

    let base = dpi.unwrap_or(BASE_DPI);
    let scale = scale_from_settings.or(env_scale).unwrap_or(1.0);
    (base * scale).clamp(48.0, 384.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xft_dpi_resource() {
        let data = b"*foreground: white\nXft.dpi: 192\nXcursor.size: 24\n";
        assert_eq!(parse_xresources(data), Some(192.0));
    }

    #[test]
    fn ignores_other_resources() {
        let data = b"Xcursor.size: 24\n";
        assert_eq!(parse_xresources(data), None);
    }

    #[test]
    fn parses_little_endian_xsettings() {
        // header: little endian, serial, 2 settings
        let mut data = vec![0u8, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0];
        // setting 1: int, name "Xft/DPI", value 98304 (96 * 1024)
        data.extend_from_slice(&[0, 0, 7, 0]);
        data.extend_from_slice(b"Xft/DPI");
        data.extend_from_slice(&[0]); // name padding to 4
        data.extend_from_slice(&98304u32.to_le_bytes());
        // setting 2: int, name "Gdk/WindowScalingFactor", value 2
        let name = b"Gdk/WindowScalingFactor";
        data.extend_from_slice(&[0, 0, name.len() as u8, 0]);
        data.extend_from_slice(name);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        data.extend_from_slice(&2u32.to_le_bytes());

        let settings = parse_xsettings(&data);
        assert_eq!(settings.len(), 2);
        assert_eq!(settings[0].name, b"Xft/DPI");
        assert_eq!(settings[0].int_value, 98304);
        assert_eq!(settings[1].name, name);
        assert_eq!(settings[1].int_value, 2);
    }
}
