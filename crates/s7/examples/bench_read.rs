//! Times Endpoint reads and writes of N tags against the in-process fake CPU.
//!   cargo run --release -p s7 --features fakecpu --example bench_read -- 100000
use std::time::Instant;

use s7::{fakecpu::FakeCpu, Endpoint, EndpointConfig, Log, Val};

fn main() {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let f = FakeCpu::new(960);
    for db in 100..104 {
        f.add_db(db, 65536);
    }
    let addr = f.start();
    let (host, port) = addr.split_once(':').unwrap();
    let cfg = EndpointConfig { host: host.into(), port: port.parse().unwrap(), cpu: "1500".into(), ..Default::default() };
    let (ep, _) = Endpoint::open("b", &cfg, &Log::discard()).unwrap();
    let tags: Vec<String> = (0..n).map(|i| format!("DB{}.DBW{}:int", 100 + i / 32768, (i % 32768) * 2)).collect();
    let items: Vec<(String, Val)> = tags.iter().enumerate().map(|(i, t)| (t.clone(), Val::Int((i % 30000) as i64))).collect();
    ep.write_values(&items).unwrap();
    let rounds = (2_000_000 / n).clamp(5, 200);
    for (name, f) in [
        ("read", Box::new(|| drop(ep.read_tags_detailed(&tags))) as Box<dyn Fn()>),
        ("write", Box::new(|| drop(ep.write_values(&items).unwrap()))),
    ] {
        f();
        let t0 = Instant::now();
        for _ in 0..rounds {
            f();
        }
        println!("{name}-{n}: {:.3} ms", t0.elapsed().as_secs_f64() * 1000.0 / rounds as f64);
    }
}
