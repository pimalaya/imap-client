use imap_client::{
    client::tokio::Client,
    imap_types::{
        fetch::{Macro, MacroOrMessageDataItemNames, MessageDataItem},
        sequence::SequenceSet,
    },
};

const USAGE: &str = "USAGE: cargo run --example=fetch-tokio-rustls --features tokio-rustls -- <host> <port> <username> <password>";

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

    let mut client = Client::rustls(host, port, false).await.unwrap();

    client.authenticate_plain(username, password).await.unwrap();

    let select_data = client.select("inbox").await.unwrap();
    println!("{select_data:?}\n");

    let data = client
        .fetch(
            SequenceSet::try_from("1:10").unwrap(),
            MacroOrMessageDataItemNames::Macro(Macro::Full),
        )
        .await
        .unwrap();

    println!("# INBOX\n");
    for (_, items) in data {
        let envelope = items
            .as_ref()
            .iter()
            .find(|item| matches!(item, MessageDataItem::Envelope(_)))
            .unwrap();
        if let MessageDataItem::Envelope(env) = envelope {
            if let Some(sub) = &env.subject.0 {
                println!("* {:?}", std::str::from_utf8(sub.as_ref()).unwrap());
            }
        }
    }
}
