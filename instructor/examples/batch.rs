use instructor_ai::{batch::BatchItem, model, BlockingClient};

#[model]
struct Person {
    name: String,
    age: u8,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().ok_or("pass provider/model")?;
    let client = BlockingClient::from_provider(&model)?;
    let batch = client.batch::<Person>()?;
    let command = args.next();
    match command.as_deref() {
        Some("prepare" | "submit") => {
            let jsonl = batch.prepare([
                BatchItem::new("Extract the person. Ali is 28 years old.").id("profile-1"),
                BatchItem::new("Extract the person. Bo is 35 years old.").id("profile-2"),
            ])?;
            if command.as_deref() == Some("submit") {
                println!("{}", batch.submit(&jsonl)?);
            } else {
                print!("{jsonl}");
            }
        }
        Some("status") => {
            let id = args.next().ok_or("pass the batch ID")?;
            println!("{}", batch.status(&id)?.status);
        }
        Some("results") => {
            let id = args.next().ok_or("pass the batch ID")?;
            for item in batch.results(&id)? {
                let key = item.custom_id.as_deref().unwrap_or("unknown");
                match item.result {
                    Ok(person) => println!("{key} {} {}", person.name, person.age),
                    Err(error) => eprintln!("{key} {} {}", error.kind, error.message),
                }
            }
        }
        _ => return Err("use prepare, submit, status <id>, or results <id>".into()),
    }
    Ok(())
}
