//! Standalone server. Configure with RI_ADDR, RI_DB and (optionally) RI_PASSWORD.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let addr = env("RI_ADDR", "0.0.0.0:8080");
    let listener = std::net::TcpListener::bind(&addr)
        .expect("cannot use RI_ADDR (is the port already in use?)");
    let password = std::env::var("RI_PASSWORD").ok().filter(|p| !p.is_empty());
    println!(
        "Resale Inventory is running. Open http://localhost:{} in your browser.",
        listener.local_addr().unwrap().port()
    );
    resale_inventory::serve(listener, env("RI_DB", "inventory.db").into(), password).await;
}
