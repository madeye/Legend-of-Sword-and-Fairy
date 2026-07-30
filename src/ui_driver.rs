//! Opt-in local HTTP driver for controlling and observing the native game.
//!
//! The server deliberately accepts loopback addresses only. It feeds key
//! transitions into the same queue as winit and captures the logical 320x200
//! RGBA frame presented by the engine.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use winit::keyboard::KeyCode;

use crate::game_loop::{render_rgba, PalColor};
use crate::surface::{Surface, SCREEN_H, SCREEN_W};

pub const DEFAULT_BIND: &str = "127.0.0.1:8765";
const TAP_DURATION: Duration = Duration::from_millis(75);

#[derive(Default)]
struct LatestFrame {
    id: u64,
    rgba: Vec<u8>,
}

/// Running UI driver. Dropping it disconnects the game from the server; the
/// listener thread itself is intentionally process-scoped.
pub(crate) struct UiDriver {
    input_rx: Receiver<(KeyCode, bool)>,
    latest_frame: Arc<RwLock<LatestFrame>>,
    capture_rgba: Vec<u8>,
    #[cfg(test)]
    local_addr: SocketAddr,
}

impl UiDriver {
    pub(crate) fn start(bind: &str) -> io::Result<Self> {
        let bind = if matches!(bind, "" | "1" | "true") {
            DEFAULT_BIND
        } else {
            bind
        };
        let addr: SocketAddr = bind.parse().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid UI driver address {bind:?}: {error}"),
            )
        })?;
        if !addr.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "UI driver must bind to a loopback address, not {}",
                    addr.ip()
                ),
            ));
        }

        let listener = TcpListener::bind(addr)?;
        let local_addr = listener.local_addr()?;
        let (input_tx, input_rx) = mpsc::channel();
        let latest_frame = Arc::new(RwLock::new(LatestFrame::default()));
        let server_frame = Arc::clone(&latest_frame);
        thread::Builder::new()
            .name("rustpal-ui-driver".into())
            .spawn(move || serve(listener, input_tx, server_frame))
            .map_err(|error| io::Error::other(format!("start UI driver thread: {error}")))?;

        eprintln!("rustpal: UI driver listening on http://{local_addr}");
        Ok(Self {
            input_rx,
            latest_frame,
            capture_rgba: vec![0; SCREEN_W * SCREEN_H * 4],
            #[cfg(test)]
            local_addr,
        })
    }

    pub(crate) fn drain_input(&self, events: &mut Vec<(KeyCode, bool)>) {
        events.extend(self.input_rx.try_iter());
    }

    pub(crate) fn capture(
        &mut self,
        surf: &Surface,
        palette: &[PalColor; 256],
        shake: Option<(u16, u16)>,
    ) {
        render_rgba(surf, palette, shake, &mut self.capture_rgba);
        if let Ok(mut frame) = self.latest_frame.write() {
            frame.id = frame.id.wrapping_add(1);
            if frame.rgba.len() == self.capture_rgba.len() {
                frame.rgba.copy_from_slice(&self.capture_rgba);
            } else {
                frame.rgba = self.capture_rgba.clone();
            }
        }
    }

    #[cfg(test)]
    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

fn serve(
    listener: TcpListener,
    input_tx: Sender<(KeyCode, bool)>,
    latest_frame: Arc<RwLock<LatestFrame>>,
) {
    for connection in listener.incoming() {
        match connection {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                if let Err(error) = handle_connection(&mut stream, &input_tx, &latest_frame) {
                    eprintln!("rustpal: UI driver request failed: {error}");
                }
            }
            Err(error) => eprintln!("rustpal: UI driver accept failed: {error}"),
        }
    }
}

fn handle_connection(
    stream: &mut TcpStream,
    input_tx: &Sender<(KeyCode, bool)>,
    latest_frame: &Arc<RwLock<LatestFrame>>,
) -> io::Result<()> {
    let mut request = [0u8; 8192];
    let size = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..size]);
    let Some(request_line) = request.lines().next() else {
        return write_text(stream, 400, "Bad Request", "empty request\n");
    };
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(path), Some(_version)) = (parts.next(), parts.next(), parts.next())
    else {
        return write_text(stream, 400, "Bad Request", "malformed request line\n");
    };
    let path = path.split('?').next().unwrap_or(path);

    if method == "OPTIONS" {
        return write_response(stream, 204, "No Content", "text/plain", &[]);
    }

    match (method, path) {
        ("GET", "/") => write_text(stream, 200, "OK", API_HELP),
        ("GET", "/v1/status") => {
            let frame = latest_frame
                .read()
                .map_err(|_| io::Error::other("frame lock poisoned"))?;
            let body = format!(
                "{{\"status\":\"ok\",\"width\":{SCREEN_W},\"height\":{SCREEN_H},\"frame_id\":{}}}\n",
                frame.id
            );
            write_response(stream, 200, "OK", "application/json", body.as_bytes())
        }
        ("GET", "/v1/frame.png") => {
            let rgba = {
                let frame = latest_frame
                    .read()
                    .map_err(|_| io::Error::other("frame lock poisoned"))?;
                if frame.rgba.is_empty() {
                    return write_text(stream, 503, "Service Unavailable", "no frame yet\n");
                }
                frame.rgba.clone()
            };
            let png = encode_png(&rgba)?;
            write_response(stream, 200, "OK", "image/png", &png)
        }
        ("POST", path) if path.starts_with("/v1/input/") => handle_input(stream, path, input_tx),
        _ => write_text(stream, 404, "Not Found", "not found\n"),
    }
}

fn handle_input(
    stream: &mut TcpStream,
    path: &str,
    input_tx: &Sender<(KeyCode, bool)>,
) -> io::Result<()> {
    let mut segments = path.trim_start_matches('/').split('/');
    let (Some("v1"), Some("input"), Some(key_name), action) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return write_text(
            stream,
            400,
            "Bad Request",
            "expected /v1/input/{key}/{action}\n",
        );
    };
    if segments.next().is_some() {
        return write_text(stream, 400, "Bad Request", "too many path segments\n");
    }
    let Some(key) = parse_key(key_name) else {
        return write_text(stream, 400, "Bad Request", "unknown key\n");
    };
    match action.unwrap_or("tap") {
        "press" => send_key(input_tx, key, true)?,
        "release" => send_key(input_tx, key, false)?,
        "tap" => {
            send_key(input_tx, key, true)?;
            thread::sleep(TAP_DURATION);
            send_key(input_tx, key, false)?;
        }
        _ => {
            return write_text(
                stream,
                400,
                "Bad Request",
                "action must be tap, press, or release\n",
            )
        }
    }
    write_response(
        stream,
        202,
        "Accepted",
        "application/json",
        b"{\"accepted\":true}\n",
    )
}

fn send_key(input_tx: &Sender<(KeyCode, bool)>, key: KeyCode, pressed: bool) -> io::Result<()> {
    input_tx
        .send((key, pressed))
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "game input queue closed"))
}

fn parse_key(name: &str) -> Option<KeyCode> {
    match name.to_ascii_lowercase().as_str() {
        "up" => Some(KeyCode::ArrowUp),
        "down" => Some(KeyCode::ArrowDown),
        "left" => Some(KeyCode::ArrowLeft),
        "right" => Some(KeyCode::ArrowRight),
        "menu" | "escape" | "esc" => Some(KeyCode::Escape),
        "confirm" | "search" | "enter" => Some(KeyCode::Enter),
        "space" => Some(KeyCode::Space),
        "page_up" | "pgup" => Some(KeyCode::PageUp),
        "page_down" | "pgdn" => Some(KeyCode::PageDown),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "repeat" | "r" => Some(KeyCode::KeyR),
        "auto" | "a" => Some(KeyCode::KeyA),
        "defend" | "d" => Some(KeyCode::KeyD),
        "use_item" | "e" => Some(KeyCode::KeyE),
        "throw_item" | "w" => Some(KeyCode::KeyW),
        "flee" | "q" => Some(KeyCode::KeyQ),
        "force" | "magic" | "f" => Some(KeyCode::KeyF),
        "status" | "s" => Some(KeyCode::KeyS),
        _ => None,
    }
}

fn encode_png(rgba: &[u8]) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, SCREEN_W as u32, SCREEN_H as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| io::Error::other(format!("encode frame header: {error}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| io::Error::other(format!("encode frame pixels: {error}")))?;
    }
    Ok(bytes)
}

fn write_text(stream: &mut TcpStream, status: u16, reason: &str, body: &str) -> io::Result<()> {
    write_response(
        stream,
        status,
        reason,
        "text/plain; charset=utf-8",
        body.as_bytes(),
    )
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

const API_HELP: &str = "\
rustpal UI driver

GET  /v1/status
GET  /v1/frame.png
POST /v1/input/{key}/tap
POST /v1/input/{key}/press
POST /v1/input/{key}/release

Keys: up, down, left, right, menu, confirm, space, page_up, page_down,
      home, end, repeat, auto, defend, use_item, throw_item, flee,
      force, status
";

#[cfg(test)]
mod tests {
    use super::*;

    fn request(addr: SocketAddr, request: &str) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).expect("connect to UI driver");
        stream
            .write_all(request.as_bytes())
            .expect("write HTTP request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read HTTP response");
        response
    }

    #[test]
    fn rejects_non_loopback_bind() {
        let error = UiDriver::start("0.0.0.0:0")
            .err()
            .expect("non-loopback bind should fail");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn status_frame_and_input_endpoints_work() {
        let mut driver = UiDriver::start("127.0.0.1:0").expect("start UI driver");
        let status = request(
            driver.local_addr(),
            "GET /v1/status HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(status.starts_with(b"HTTP/1.1 200 OK"));
        assert!(status
            .windows(b"\"frame_id\":0".len())
            .any(|part| part == b"\"frame_id\":0"));

        let mut surface = Surface::screen();
        surface.pixels.fill(1);
        let mut palette = [[0; 3]; 256];
        palette[1] = [12, 34, 56];
        driver.capture(&surface, &palette, None);
        let frame = request(
            driver.local_addr(),
            "GET /v1/frame.png HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(frame.starts_with(b"HTTP/1.1 200 OK"));
        assert!(frame.windows(8).any(|part| part == b"\x89PNG\r\n\x1a\n"));

        let input = request(
            driver.local_addr(),
            "POST /v1/input/confirm/tap HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        );
        assert!(input.starts_with(b"HTTP/1.1 202 Accepted"));
        let mut events = Vec::new();
        driver.drain_input(&mut events);
        assert_eq!(events, [(KeyCode::Enter, true), (KeyCode::Enter, false)]);
    }
}
