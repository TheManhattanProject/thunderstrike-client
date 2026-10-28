//! The binding against the library, and (with THUNDERSTRIKE_TEST_ADDR) a server.
//!
//! THUNDERSTRIKE_TEST_ADDR=host:port runs the server tests. With
//! THUNDERSTRIKE_CA (and THUNDERSTRIKE_SERVER_NAME) they use TLS, as the real
//! server requires; without it they assume a plaintext stand-in.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use thunderstrike::{code, hash_shard, Client, Config, RetryClass};

// The routing hash is the canary for talking to the same implementation as
// every other binding: user:1 is shard 104 of 1024.
#[test]
fn hash_shard_is_pinned() {
    assert_eq!(hash_shard(b"user:1", 1024).unwrap(), 104);
}

#[test]
fn retry_classes_follow_the_abi() {
    assert_eq!(RetryClass::of(code::BUSY), RetryClass::Always);
    for c in [code::TIMEOUT, code::DISCONNECTED, code::CONNECT, code::IO] {
        assert_eq!(RetryClass::of(c), RetryClass::IfIdempotent, "code {c}");
    }
    for c in [code::SERVER_OOM, code::STORAGE_EXHAUSTED, code::SERVER_ERROR, code::MALFORMED, code::TLS, code::CLOSED] {
        assert_eq!(RetryClass::of(c), RetryClass::Never, "code {c}");
    }
    // The codes are the wire status bytes, and TLS moved off 13.
    assert_eq!((code::BUSY, code::SERVER_ERROR, code::STORAGE_EXHAUSTED, code::TLS), (13, 14, 12, -13));
}

#[test]
fn version_and_strerror_are_strings() {
    assert!(!thunderstrike::version().is_empty());
    assert!(!thunderstrike::strerror(code::BUSY).is_empty());
}

// The server is TLS-only: a config with neither TLS nor plaintext is refused.
#[test]
fn plaintext_must_be_asked_for() {
    let e = Client::connect(&Config::new("127.0.0.1:1")).unwrap_err();
    assert_eq!(e.code(), code::CONFIG, "{e}");
}

#[test]
fn nul_in_an_endpoint_is_an_argument_error() {
    let e = Client::connect(&Config::new("127.0.0.1\0:1").insecure_plaintext()).unwrap_err();
    assert_eq!(e.code(), code::INVALID_ARG);
}

fn server() -> Option<Client> {
    let addr = std::env::var("THUNDERSTRIKE_TEST_ADDR").ok()?;
    let cfg = Config { connections_per_endpoint: Some(2), ..Config::new(addr) };
    let cfg = match std::env::var_os("THUNDERSTRIKE_CA") {
        Some(ca) => {
            let name = std::env::var("THUNDERSTRIKE_SERVER_NAME").unwrap_or_else(|_| "localhost".into());
            cfg.tls(name, std::fs::read(ca).unwrap())
        }
        None => cfg.insecure_plaintext(),
    };
    Some(Client::connect(&cfg).expect("connect"))
}

macro_rules! need_server {
    () => {
        match server() {
            Some(db) => db,
            None => {
                eprintln!("skipped: set THUNDERSTRIKE_TEST_ADDR");
                return;
            }
        }
    };
}

#[test]
fn round_trips() {
    let db = need_server!();
    db.ping().unwrap();
    for (i, n) in [1usize, 100, 4097, 65537, 1_048_577, (4 << 20) + 1].into_iter().enumerate() {
        let k = format!("rs:test:{n}").into_bytes();
        let v: Vec<u8> = (0..n).map(|j| (j * 131 + i * 7 + n) as u8).collect();
        db.set(&k, &v).unwrap();
        assert_eq!(db.get(&k).unwrap().as_deref(), Some(&v[..]), "get {n}");
        assert!(db.exists(&k).unwrap());
        assert_eq!(db.append(&k, b"tail").unwrap(), (n + 4) as u64, "append {n}");

        let mut small = [0u8; 4];
        assert_eq!(db.get_into(&k, &mut small).unwrap(), Some(n + 4), "get_into reports the true length");
        let mut exact = vec![0u8; n + 4];
        assert_eq!(db.get_into(&k, &mut exact).unwrap(), Some(n + 4));
        assert_eq!(&exact[..n], &v[..]);
        assert_eq!(&exact[n..], b"tail");

        let many = db.mget(&[&k[..], b"rs:test:absent"]).unwrap();
        assert_eq!(many[0].as_ref().map(Vec::len), Some(n + 4));
        assert_eq!(many[1], None);
        assert!(db.delete(&k).unwrap(), "delete {n}");
        assert_eq!(db.get(&k).unwrap(), None);
        assert!(!db.delete(&k).unwrap());
        assert!(!db.exists(&k).unwrap());
    }
}

#[test]
fn batches() {
    let db = need_server!();
    let keys: Vec<Vec<u8>> = (0..64).map(|i| format!("rs:batch:{i}").into_bytes()).collect();
    let pairs: Vec<(&[u8], Vec<u8>)> = keys.iter().map(|k| (&k[..], [k.as_slice(), b"=v"].concat())).collect();
    db.mset(&pairs).unwrap();
    let got = db.mget(&keys).unwrap();
    for ((k, v), g) in pairs.iter().zip(&got) {
        assert_eq!(g.as_deref(), Some(&v[..]), "{}", String::from_utf8_lossy(k));
    }
    assert!(db.mdelete(&keys).unwrap().into_iter().all(|d| d));
    assert!(db.mdelete(&keys).unwrap().into_iter().all(|d| !d));
    assert_eq!(db.mget::<&[u8]>(&[]).unwrap(), Vec::<Option<Vec<u8>>>::new());
}

// The server refuses an empty value: permanent, not retriable.
#[test]
fn empty_value_is_malformed() {
    let db = need_server!();
    let e = db.set(b"rs:empty", b"").unwrap_err();
    assert_eq!(e.code(), code::MALFORMED, "{e}");
    assert_eq!(e.retry_class(), RetryClass::Never);
}

#[test]
fn introspection() {
    let db = need_server!();
    db.ping().unwrap();
    let s = db.stats().unwrap();
    assert!(s.connections >= 1 && s.connections_ready >= 1, "{s:?}");
    assert!(s.requests >= 1);
    db.mux_stats().unwrap();
    assert!(db.endpoint_count().unwrap() >= 1);
    let shards = db.shard_count().unwrap();
    if shards > 0 {
        assert_eq!(db.shard_of(b"user:1").unwrap(), hash_shard(b"user:1", shards).unwrap());
    }
}

/// A minimal executor: the futures must work without any runtime.
fn block_on<F: Future>(f: F) -> F::Output {
    struct Unpark(Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park();
    }
}

#[test]
fn async_round_trip() {
    let db = need_server!();
    block_on(db.set_async(b"rs:async", b"value")).unwrap();
    assert_eq!(block_on(db.get_async(b"rs:async")).unwrap().as_deref(), Some(&b"value"[..]));
    assert!(block_on(db.delete_async(b"rs:async")).unwrap());
    assert!(!block_on(db.delete_async(b"rs:async")).unwrap());
    assert_eq!(block_on(db.get_async(b"rs:async")).unwrap(), None);

    // Many in flight at once, awaited in reverse.
    let sets: Vec<_> = (0..256).map(|i| db.set_async(format!("rs:async:{i}").as_bytes(), b"v")).collect();
    for f in sets.into_iter().rev() {
        block_on(f).unwrap();
    }
    let e = block_on(db.set_async(b"rs:async:empty", b"")).unwrap_err();
    assert_eq!(e.code(), code::MALFORMED);
    for i in 0..256 {
        db.delete(format!("rs:async:{i}").as_bytes()).unwrap();
    }
}

// Dropping the client with requests in flight resolves them; nothing hangs.
#[test]
fn drop_with_requests_in_flight() {
    let db = need_server!();
    let pending: Vec<_> = (0..512).map(|i| db.get_async(format!("rs:drop:{i}").as_bytes())).collect();
    drop(db);
    for f in pending {
        match block_on(f) {
            Ok(None) => {}
            Err(e) => assert!(matches!(e.code(), code::CLOSED | code::DISCONNECTED), "{e}"),
            Ok(Some(_)) => panic!("absent key returned a value"),
        }
    }
}

// Shared across threads by reference.
#[test]
fn concurrent_use() {
    let db = need_server!();
    thread::scope(|s| {
        for t in 0..8 {
            let db = &db;
            s.spawn(move || {
                for i in 0..200 {
                    let k = format!("rs:conc:{t}:{i}");
                    db.set(k.as_bytes(), k.as_bytes()).unwrap();
                    assert_eq!(db.get(k.as_bytes()).unwrap().as_deref(), Some(k.as_bytes()));
                    db.delete(k.as_bytes()).unwrap();
                }
            });
        }
    });
}
