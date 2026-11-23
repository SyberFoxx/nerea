//! Talking to the compositor, cheaply.
//!
//! Two things push against each other here. Wayland forbids a client from
//! placing its own window, so moving her is the compositor's job. But the
//! native way to ask for that — a drag region, or `start_dragging` — only works
//! while the button press is being handled, and it then grabs the pointer, so
//! every later press goes to the compositor and a plain click can never be
//! detected. Dragging her and clicking her cannot both ride on that path.
//!
//! So the movement is driven from here instead: while the pointer travels, the
//! window is placed where it should be. Same dispatcher the docking code
//! already uses, only through the command socket instead of one `hyprctl`
//! process per mouse move.
//!
//! Everything is optional: a compositor without these sockets reports
//! [`available`] as false, and the UI keeps the native drag region instead.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::Mutex,
    time::Duration,
};

/// Where a drag started: the window's corner and the pointer that grabbed it.
#[derive(Debug, Clone)]
struct Grab {
    address: String,
    origin: (i32, i32),
    cursor: (f64, f64),
}

static GRAB: Mutex<Option<Grab>> = Mutex::new(None);

fn socket_dir() -> Option<PathBuf> {
    let signature = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let dir = runtime.join("hypr").join(signature);
    dir.exists().then_some(dir)
}

fn socket_path() -> Option<PathBuf> {
    let path = socket_dir()?.join(".socket.sock");
    path.exists().then_some(path)
}

/// One request on the command socket: no process, no polling.
fn command(request: &str) -> Option<String> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;

    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply)
}

/// Whether this compositor can be driven. False on anything but Hyprland.
pub fn available() -> bool {
    socket_path().is_some()
}

/// The pointer's position on the layout, straight from the compositor.
fn cursor() -> Option<(f64, f64)> {
    let reply = command("cursorpos")?;
    let (x, y) = reply.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// A window address as Hyprland prints it: `0x` and a run of hex digits.
fn parse_address(text: &str) -> Option<String> {
    let text = text.trim();
    let hex = text.strip_prefix("0x")?;
    if hex.is_empty() || hex.len() > 16 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(text.to_owned())
}

fn move_to(address: &str, x: i32, y: i32) -> bool {
    let request = format!(
        "dispatch hl.dsp.window.move({{ x = {x}, y = {y}, window = \"address:{address}\" }})"
    );
    command(&request).is_some()
}

/// Keeps her out of the tiling layout. A fixed-size window floats on its own;
/// a resizable one has to be asked.
pub fn float(address: &str) -> bool {
    let Some(address) = parse_address(address) else {
        return false;
    };
    let request =
        format!("dispatch hl.dsp.window.float({{ action = \"enable\", window = \"address:{address}\" }})");
    command(&request).is_some()
}

/// Remembers where the window and the pointer are when a drag starts.
pub fn begin(address: &str, origin: (i32, i32)) -> Result<(), String> {
    let address = parse_address(address).ok_or("unreadable window address")?;
    let cursor = cursor().ok_or("could not read the pointer")?;

    let mut guard = GRAB.lock().map_err(|_| "drag state poisoned".to_owned())?;
    *guard = Some(Grab { address, origin, cursor });
    Ok(())
}

/// Places the window where the pointer has taken it. Cheap enough to run often.
pub fn step() -> Result<(), String> {
    let grab = {
        let guard = GRAB.lock().map_err(|_| "drag state poisoned".to_owned())?;
        guard.clone().ok_or("no drag in progress")?
    };

    let cursor = cursor().ok_or("could not read the pointer")?;
    let dx = (cursor.0 - grab.cursor.0).round() as i32;
    let dy = (cursor.1 - grab.cursor.1).round() as i32;
    if dx == 0 && dy == 0 {
        return Ok(());
    }

    if move_to(&grab.address, grab.origin.0 + dx, grab.origin.1 + dy) {
        Ok(())
    } else {
        Err("the compositor refused to move the window".to_owned())
    }
}

pub fn end() {
    if let Ok(mut guard) = GRAB.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_address_is_hex_behind_a_prefix() {
        assert_eq!(parse_address("0x560875af3e40").as_deref(), Some("0x560875af3e40"));
        assert_eq!(parse_address("  0xdeadbeef  ").as_deref(), Some("0xdeadbeef"));
    }

    #[test]
    fn nonsense_addresses_are_refused() {
        assert!(parse_address("0x123").is_some(), "short but valid hex");
        assert!(parse_address("0x").is_none());
        assert!(parse_address("nope").is_none());
        assert!(parse_address("").is_none());
        assert!(parse_address("0xzz").is_none());
    }

    #[test]
    fn stepping_without_a_grab_is_an_error_not_a_panic() {
        end();
        assert!(step().is_err());
    }

    #[test]
    fn beginning_a_drag_needs_a_real_window() {
        assert!(begin("nope", (0, 0)).is_err());
    }
}
