use instructor_ai::{model, BlockingClient};

#[model]
struct Person {
    /// Full name as written in the source.
    name: String,
    age: u8,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = std::env::args().nth(1).ok_or("pass provider/model")?;
    let client = BlockingClient::from_provider(&model)?;
    let person: Person = client.create("Extract the person. Ada Chen is 37 years old.")?;
    println!("{} is {}", person.name, person.age);
    Ok(())
}
