use instructor_ai::{model, streaming, Error, PartialEvent};

#[model]
struct Person {
    name: String,
    age: u8,
}

fn main() -> Result<(), Error> {
    let fragments = [r#"{"name":"Ad"#, r#"a","age":"#, "37}"];
    for event in streaming::partial::<Person>(fragments.map(Ok)) {
        match event? {
            PartialEvent::Partial(person) => {
                println!("Preview {:?}, {:?}", person.name, person.age);
            }
            PartialEvent::Complete(person) => {
                println!("Complete {}, {}", person.name, person.age);
            }
        }
    }
    Ok(())
}
