use imap_client::client::tokio::Client;

const USAGE: &str = "USAGE: cargo run --example append -- <host> <port> <username> <password>";

#[tokio::main]
async fn main() {
    let (host, port, username, password) = {
        let mut args = std::env::args();
        let _ = args.next();

        (
            args.next().expect(USAGE),
            str::parse::<u16>(&args.next().expect(USAGE)).unwrap(),
            args.next().expect(USAGE),
            args.next().expect(USAGE),
        )
    };

    let mut client = Client::rustls(host, port, false, None).await.unwrap();

    client.authenticate_plain(username, password).await.unwrap();

    client.select("Drafts").await.unwrap();

    println!("append");

    let data = client
        .append("Drafts", [], include_bytes!("../message.eml"))
        .await
        .unwrap();

    println!("data: {data:?}");
}
