//! `hwc-lobby`: the lobby server for internet games (in place of the original's WON servers).
//! Players log in, chat in rooms, list the games hosted in their room and join them, directly
//! or through this server's relay when the host cannot take connections.
//!
//! ```text
//! hwc-lobby [--bind ADDRESS:PORT] [--build BUILD]
//! ```
//!
//! The default address is every interface on port 28472; the build defaults to this program's
//! (players must run the same one).

use std::time::Duration;

fn main() {
    let mut bind = format!("0.0.0.0:{}", hwc_net::internet::LOBBY_PORT);
    let mut build = hwc_net::build_id();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => bind = args.next().unwrap_or(bind),
            "--build" => build = args.next().unwrap_or(build),
            "-h" | "--help" => {
                println!("hwc-lobby [--bind ADDRESS:PORT] [--build BUILD]");
                return;
            }
            other => {
                eprintln!("hwc-lobby: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let mut server = match hwc_net::internet::LobbyServer::bind(&bind, &build) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("hwc-lobby: cannot listen on {bind}: {e}");
            std::process::exit(1);
        }
    };
    println!("hwc-lobby: listening on {} for {build}", server.local_addr().map(|a| a.to_string()).unwrap_or(bind));
    loop {
        server.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
}
