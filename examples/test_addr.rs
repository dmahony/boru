use iroh::{endpoint::presets, Endpoint, RelayMode, SecretKey};

#[tokio::main]
async fn main() {
    let sk = SecretKey::generate();
    let pk = sk.public();

    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();

    let addr = ep.addr();
    println!("ep.addr() = {:?}", addr);
    println!("ep.addr().id = {:?}", addr.id);
    println!(
        "ep.addr().relay_urls = {:?}",
        addr.relay_urls().collect::<Vec<_>>()
    );
    println!(
        "ep.addr().ip_addrs = {:?}",
        addr.ip_addrs().collect::<Vec<_>>()
    );

    let addr2 = iroh::EndpointAddr::new(pk);
    println!("EndpointAddr::new(pk) = {:?}", addr2);
    println!(
        "EndpointAddr::new(pk) relay_urls = {:?}",
        addr2.relay_urls().collect::<Vec<_>>()
    );
    println!(
        "EndpointAddr::new(pk) ip_addrs = {:?}",
        addr2.ip_addrs().collect::<Vec<_>>()
    );

    drop(ep);
}
