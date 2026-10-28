//! The client in one program.
//!
//! ```text
//! cargo run --example basic -- 127.0.0.1:7379
//! THUNDERSTRIKE_CA=ca.crt THUNDERSTRIKE_SERVER_NAME=localhost cargo run --example basic -- db:7379
//! ```
//!
//! Without THUNDERSTRIKE_CA it speaks plaintext, which only a local stand-in
//! server answers; the thunderstrike server is TLS 1.3 only.

use std::time::Duration;
use thunderstrike::{Client, Config};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:7379".into());
    let cfg = match std::env::var_os("THUNDERSTRIKE_CA") {
        Some(ca) => {
            let name = std::env::var("THUNDERSTRIKE_SERVER_NAME").unwrap_or_else(|_| "localhost".into());
            Config::new(&addr).tls(name, std::fs::read(ca)?)
        }
        None => Config::new(&addr).insecure_plaintext(),
    };

    println!("thunderstrike {}", thunderstrike::version());
    let db = Client::connect(&cfg)?;
    db.ping()?;

    db.set(b"example:greeting", b"hello")?;
    let total = db.append(b"example:greeting", b", world")?;
    let v = db.get(b"example:greeting")?.unwrap_or_default();
    println!("get -> {:?} ({total} bytes)", String::from_utf8_lossy(&v));

    db.set_ttl(b"example:session", b"token", Duration::from_secs(60))?;
    println!("exists -> {}", db.exists(b"example:session")?);

    // Batches: n keys, one round trip.
    db.mset(&[(&b"example:a"[..], &b"1"[..]), (b"example:b", b"2")])?;
    let many = db.mget(&[&b"example:a"[..], b"example:b", b"example:absent"])?;
    println!("mget -> {many:?}");
    println!("mdelete -> {:?}", db.mdelete(&[&b"example:a"[..], b"example:b"])?);

    match db.set(b"example:empty", b"") {
        Err(e) => println!("empty set refused: {e} (retry: {:?})", e.retry_class()),
        Ok(()) => println!("empty set accepted"),
    }

    let s = db.stats()?;
    println!(
        "{} connections, {} requests, batching x{:.2}; shard of user:1 = {}",
        s.connections,
        s.requests,
        s.batching_factor(),
        thunderstrike::hash_shard(b"user:1", 1024)?
    );
    db.delete(b"example:greeting")?;
    Ok(())
}
