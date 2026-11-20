//! A WebRTC Direct listener in front of a full engine: the reference host
//! loop the native bridges copy, and what a page or a phone dials in tests.
//!
//! ```text
//! cargo run -p offline-protocol-webrtc-direct --example listener -- <private-ip> [--port N] [--app-id ID]
//! ```
//!
//! It binds a UDP socket on `<private-ip>` (port 0, so a fresh one, unless
//! `--port` names one), starts an engine with a fresh identity over
//! in-memory stores carrying the peer-stream slot only, opens a listener
//! with the engine's address and identity assertion, and reports the slot
//! up. `--app-id` defaults to `webrtc-direct-example`.
//!
//! # Standard output
//!
//! One JSON object per line, each with a `type`. The first line is always
//! `listening`; the rest follow as things happen. Keys within a line are in
//! no particular order.
//!
//! | `type` | Fields | When |
//! |--------|--------|------|
//! | `listening` | `code` (the fragment, `op-direct=1&...`), `address`, `host`, `port` | First, once |
//! | `proved` | `stream`, `address`, `remote` (`ip:port`), `superseded` (a stream id or `null`) | A dial's preamble verified and the bridge kept it. With `superseded`, it replaced an older dialled stream for the same address, and the engine was not told again |
//! | `refused` | `stream`, `address`, `reason` | A proved dial was closed and never announced (a cap, or this device's own address) |
//! | `lost` | `stream`, `address` | The stream holding `address` ended and the engine was told |
//! | `dropped` | `to`, `reason` | An outbound body had no stream, or the listener refused it |
//! | `neighbor_discovered` | `peer` | The engine was told a stream proved `peer` |
//! | `neighbor_lost` | `peer` | The engine was told the stream for `peer` ended |
//! | `session_ready` | `peer` | An MLS session with `peer` is established |
//! | `accepted` | `peer` | A connection request from `peer` was accepted |
//! | `message` | `id`, `from`, `len`, `encrypted`, `text` (only when `len` <= 4096) | A direct message arrived. Unless its text starts with `echo:`, the host replies `echo:` + the text to `from` |
//! | `sent` | `id`, `to`, `len` | The engine accepted a send (an echo or a command) |
//! | `delivered` | `id` | A sent message was acknowledged |
//! | `failed` | `id`, `reason` | A sent message failed |
//! | `closed` | | The listener closed (the `close` command) |
//! | `error` | `reason` | A command or a send failed |
//!
//! Diagnostics go to standard error.
//!
//! # Standard input
//!
//! One command per line:
//!
//! - `close`: closes the listener. Every stream ends and the engine hears
//!   each loss (`lost`, `neighbor_lost`), then `closed`. The code is dead;
//!   the process keeps running until `quit`.
//! - `send <address> <text>`: a direct message; the text is the rest of the
//!   line.
//! - `big <address> <n>`: a direct message of `n` bytes of `x`.
//! - `quit`: exits.
//!
//! The end of standard input is not `quit`: the host keeps running.
//!
//! # The loop
//!
//! One thread owns the socket, the listener and the engine. It waits on the
//! socket until a datagram arrives or the next timer is due (the listener's
//! own, or the engine's 100 ms tick), then hands over the datagram, runs
//! what is due, takes commands from a helper thread that reads standard
//! input, sends what the listener queued, and prints.

mod bridge;
mod engine;
mod host;

use std::io::{BufRead, ErrorKind};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::mpsc::{self, Receiver};

use offline_protocol_core::time::{Duration, Instant};

use crate::host::{Command, Host, Line};

const USAGE: &str = "usage: listener <private-ip> [--port N] [--app-id ID]";

struct Args {
    ip: IpAddr,
    port: u16,
    app_id: String,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let mut ip = None;
        let mut port = 0;
        let mut app_id = "webrtc-direct-example".to_string();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => {
                    port = args
                        .next()
                        .and_then(|p| p.parse().ok())
                        .ok_or("--port takes a number")?;
                }
                "--app-id" => app_id = args.next().ok_or("--app-id takes a value")?,
                _ if ip.is_none() => {
                    ip = Some(
                        arg.parse()
                            .map_err(|_| format!("not an IP address: {arg}"))?,
                    );
                }
                _ => return Err(USAGE.to_string()),
            }
        }
        Ok(Self {
            ip: ip.ok_or(USAGE)?,
            port,
            app_id,
        })
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("listener: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    let socket = UdpSocket::bind(SocketAddr::new(args.ip, args.port))
        .map_err(|e| format!("bind {}: {e}", args.ip))?;
    // The real port goes in the code, never 0.
    let local = socket.local_addr().map_err(|e| e.to_string())?;
    let mut host = Host::open(&args.app_id, "listener", local, Instant::now())?;
    print(&host.listening());

    let commands = read_stdin();
    let mut buffer = vec![0u8; 65_536];
    loop {
        let wait = host
            .next_wake()
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        socket
            .set_read_timeout(Some(wait))
            .map_err(|e| e.to_string())?;
        match socket.recv_from(&mut buffer) {
            Ok((len, source)) => {
                if let Some(bytes) = buffer.get(..len) {
                    host.handle_datagram(Instant::now(), source, bytes);
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            // An ICMP unreachable for an earlier send surfaces here on some
            // platforms; it is about one page, not the socket.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => {}
            Err(e) => return Err(format!("receive: {e}")),
        }

        let mut lines = host.turn(Instant::now());
        let mut quit = false;
        // A closed standard input is not `quit`: the helper thread just ends.
        while let Ok(input) = commands.try_recv() {
            match host.command(&input) {
                Command::Continue(more) => lines.extend(more),
                Command::Quit => {
                    quit = true;
                    break;
                }
            }
        }
        while let Some(transmit) = host.poll_transmit() {
            if let Err(e) = socket.send_to(&transmit.bytes, transmit.destination) {
                eprintln!("send to {}: {e}", transmit.destination);
            }
        }
        for line in &lines {
            print(line);
        }
        if quit {
            return Ok(());
        }
    }
}

fn print(line: &Line) {
    println!("{}", line.0);
}

/// Standard input, a line at a time, from a helper thread.
fn read_stdin() -> Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                return;
            };
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    receiver
}
