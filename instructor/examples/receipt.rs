use instructor_ai::{model, BlockingClient, Media, Message};

#[model]
struct Receipt {
    merchant: String,
    /// The total in the smallest unit of the given currency.
    total_minor_units: u64,
    currency: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().ok_or("pass provider/model")?;
    let path = args.next().ok_or("pass a path to a PNG receipt")?;
    let attachment = Media::bytes("image/png", std::fs::read(path)?);
    let message = Message::user("Read the merchant, total and currency.").attach(attachment);
    let client = BlockingClient::from_provider(&model)?;
    let receipt: Receipt = client.create(vec![message])?;
    println!(
        "{} charged {} minor units of {}",
        receipt.merchant, receipt.total_minor_units, receipt.currency
    );
    Ok(())
}
