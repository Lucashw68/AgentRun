use std::{env, io::Write, net::TcpListener, process::Command, thread, time::Duration};

unsafe extern "C" { fn signal(signum: i32, handler: usize) -> usize; }

fn main() {
    let args: Vec<_> = env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("basic");
    if mode == "exit" { return; }
    if mode == "flood" {
        for _ in 0..320 { std::io::stdout().write_all(&[b'x'; 16384]).unwrap(); }
        println!("\nFLOOD_DONE");
    }
    if mode == "stubborn" || mode == "stubborn-tree" {
        // SAFETY: Linux SIGTERM=15, SIG_IGN=1; no Rust callback crosses FFI.
        unsafe { signal(15, 1); }
    }
    if mode == "tree" || mode == "stubborn-tree" || mode == "leader-exits" {
        let child = Command::new(env::current_exe().unwrap()).arg(if mode == "tree" { "server" } else { "stubborn" }).spawn().unwrap();
        println!("CHILD={}", child.id());
    }
    let _server = if mode == "server" {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        println!("PORT={}", server.local_addr().unwrap().port());
        Some(server)
    } else { None };
    if mode == "literal" { println!("LITERAL={}", args[2]); }
    if mode == "verify-registry" {
        let registry = std::fs::read_to_string(&args[2]).expect("registry must exist before exec");
        assert!(registry.contains(&format!("\"pid\": {}", std::process::id())));
        println!("REGISTERED_BEFORE_EXEC");
    }
    println!("READY={}", std::process::id());
    eprintln!("STDERR ready");
    std::io::stdout().flush().unwrap();
    loop { thread::sleep(Duration::from_secs(1)); }
}
